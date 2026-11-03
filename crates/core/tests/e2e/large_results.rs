//! 大量データ: 取得上限、一時停止と再開、メモリ上限、退避、並べ替え。

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_limit_pauses_resumes_and_stops() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let opts = ExecuteOptions {
        fetch_limit: FetchLimit::Rows(1000),
        ..Default::default()
    };
    h.send_exec(
        tab,
        "select g from generate_series(1, 5000) g",
        ExecuteTarget::All,
        opts,
    );

    let evs = h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let result = match evs.last().unwrap() {
        Event::FetchPaused { result, rows } => {
            assert_eq!(*rows, 1000);
            *result
        }
        _ => unreachable!(),
    };
    let handle = result_handle(&evs);
    // 一時停止の状態は、FetchPaused と前後して通知される
    assert!(
        evs.iter().any(|e| matches!(e, Event::RunStateChanged { state: RunState::Paused { result: r }, .. } if *r == result)),
        "{evs:#?}"
    );
    assert_eq!(
        handle.row_count(),
        1000,
        "exactly the limit is held while paused"
    );
    assert!(!handle.can_sort(), "a partial result must not be sortable");

    // 並べ替えは受け付けない
    h.core.send(Command::SortResult {
        result,
        column: 0,
        order: SortOrder::Descending,
    });
    h.until(|e| {
        matches!(
            e,
            Event::CommandFailed {
                command: "SortResult",
                ..
            }
        )
    })
    .await;

    // 続きを取得すると、さらに 1000 行ぶんで止まる
    h.core.send(Command::FetchMore {
        result,
        more: FetchMore::NextBatch,
    });
    h.until(|e| matches!(e, Event::FetchPaused { rows: 2000, .. }))
        .await;
    assert_eq!(handle.row_count(), 2000);

    // 打ち切る。後続の文があれば続けて実行される
    h.core.send(Command::StopFetch { result });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    let outs = outcomes(&evs);
    let [StatementOutcome::Truncated { rows, reason, .. }] = outs.as_slice() else {
        panic!("{evs:?}")
    };
    assert_eq!((*rows, *reason), (2000, TruncateReason::UserStopped));
    assert!(handle.is_finished() && handle.is_truncated() && !handle.can_sort());
    // 受信できた行は読める
    assert_eq!(rows_text(&handle, 1999), "2000");

    let evs = h.exec(tab, "select 1").await;
    assert_eq!(
        status(&evs),
        ExecutionStatus::Completed,
        "session is free again"
    );
}

fn rows_text(h: &ResultHandle, i: u64) -> String {
    String::from_utf8(h.read_row_blocking(i).unwrap().unwrap()[0].clone().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_the_rest_completes_and_enables_sorting() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let opts = ExecuteOptions {
        fetch_limit: FetchLimit::Rows(100),
        ..Default::default()
    };
    h.send_exec(
        tab,
        "select g, g::numeric / 8 as frac, 'x' || g as txt from generate_series(1, 2500) g",
        ExecuteTarget::All,
        opts,
    );
    let evs = h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let Event::FetchPaused { result, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    let handle = result_handle(&evs);
    h.core.send(Command::FetchMore {
        result,
        more: FetchMore::Rest,
    });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert!(matches!(
        outcomes(&evs).as_slice(),
        [StatementOutcome::Completed { rows: 2500, .. }]
    ));
    assert!(handle.can_sort());

    // 数値として降順(文字列順なら "999" が先頭になる)
    h.core.send(Command::SortResult {
        result,
        column: 0,
        order: SortOrder::Descending,
    });
    h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(handle.sort_spec(), Some((0, SortOrder::Descending)));
    let top = handle.with_row(0, |r| r.cell(0).map(<[u8]>::to_vec));
    assert_eq!(top, Access::Ready(Some(b"2500".to_vec())));
    let last = handle.with_row(2499, |r| r.cell(0).map(<[u8]>::to_vec));
    assert_eq!(last, Access::Ready(Some(b"1".to_vec())));

    // 小数も f64 ではなく 10 進数として並ぶ。文字列の列は文字列順
    h.core.send(Command::SortResult {
        result,
        column: 1,
        order: SortOrder::Ascending,
    });
    h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.read_row_blocking(0).unwrap().unwrap()[1].as_deref(),
        Some(&b"0.12500000000000000000"[..]),
        "the server's text is kept as is"
    );
    h.core.send(Command::SortResult {
        result,
        column: 2,
        order: SortOrder::Ascending,
    });
    h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.read_row_blocking(0).unwrap().unwrap()[2].as_deref(),
        Some(&b"x1"[..])
    );
    assert_eq!(
        handle.read_row_blocking(1).unwrap().unwrap()[2].as_deref(),
        Some(&b"x10"[..])
    );

    h.core.send(Command::ClearSort { result });
    h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(handle.sort_spec(), None);
    assert_eq!(rows_text(&handle, 0), "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_while_paused_truncates_and_ends_the_batch() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let opts = ExecuteOptions {
        fetch_limit: FetchLimit::Rows(500),
        ..Default::default()
    };
    h.send_exec(
        tab,
        "select g from generate_series(1, 100000) g; select 2",
        ExecuteTarget::All,
        opts,
    );
    h.until(|e| {
        matches!(
            e,
            Event::RunStateChanged {
                state: RunState::Paused { .. },
                ..
            }
        )
    })
    .await;
    h.core.send(Command::Cancel { tab });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert!(matches!(
        outcomes(&evs).as_slice(),
        [StatementOutcome::Truncated {
            reason: TruncateReason::UserStopped,
            ..
        }]
    ));
    assert_eq!(status(&evs), ExecutionStatus::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_fetch_limit_streams_everything() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h
        .exec_with(
            tab,
            "select g from generate_series(1, 250000) g",
            ExecuteTarget::All,
            ExecuteOptions {
                fetch_limit: FetchLimit::Unlimited,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        outcomes(&evs).as_slice(),
        [StatementOutcome::Completed { rows: 250_000, .. }]
    ));
    assert!(!evs.iter().any(|e| matches!(e, Event::FetchPaused { .. })));
    // 進捗は間引かれている(25 万行でも数百件を超えない)
    let progress = evs
        .iter()
        .filter(|e| matches!(e, Event::Progress { .. }))
        .count();
    assert!(progress < 500, "{progress} progress events");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_fetch_limit_is_one_hundred_thousand_rows() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(
        tab,
        "select g from generate_series(1, 150000) g",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    assert!(matches!(
        evs.last().unwrap(),
        Event::FetchPaused { rows: 100_000, .. }
    ));
    h.core.send(Command::Cancel { tab });
    h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_spill_the_memory_limit_truncates_the_result() {
    let db = pg!();
    let mut h = H::start_with(db, |c| {
        c.memory_limit = 256 * 1024;
        c.spill_dir = None;
    })
    .await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h
        .exec_with(
            tab,
            "select g, repeat('x', 100) from generate_series(1, 100000) g",
            ExecuteTarget::All,
            ExecuteOptions {
                fetch_limit: FetchLimit::Unlimited,
                ..Default::default()
            },
        )
        .await;
    let outs = outcomes(&evs);
    let [StatementOutcome::Truncated { rows, reason, .. }] = outs.as_slice() else {
        panic!("{evs:?}")
    };
    assert_eq!(*reason, TruncateReason::MemoryLimit);
    assert!(*rows > 0 && *rows < 100_000);
    let handle = result_handle(&evs);
    assert_eq!(handle.row_count(), *rows);
    assert!(!handle.can_sort());
    assert_eq!(rows_text(&handle, 0), "1");
    // 打ち切りはエラーではない。タブはそのまま使える
    let evs = h.exec(tab, "select 1").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_spill_large_results_survive_the_memory_limit() {
    let db = pg!();
    let spill = tempfile::tempdir().unwrap();
    let spill_dir = spill.path().join("spill");
    let mut h = H::start_with(db, |c| {
        c.memory_limit = 512 * 1024;
        c.spill_dir = Some(spill_dir.clone());
    })
    .await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h
        .exec_with(
            tab,
            "select g, repeat('y', 100), g % 7 = 0, case when g % 5 = 0 then null else g::text end from generate_series(1, 60000) g",
            ExecuteTarget::All,
            ExecuteOptions { fetch_limit: FetchLimit::Unlimited, ..Default::default() },
        )
        .await;
    assert!(matches!(
        outcomes(&evs).as_slice(),
        [StatementOutcome::Completed { rows: 60_000, .. }]
    ));
    let handle = result_handle(&evs);
    assert!(handle.can_sort());
    let stats = handle.stats();
    assert!(
        stats.spilled_bytes > 0,
        "memory limit must have forced a spill"
    );
    assert!(
        stats.resident_bytes < 4 * 1024 * 1024,
        "resident {}",
        stats.resident_bytes
    );

    // 退避した先頭の行も、読み戻して正しく読める
    for i in [0u64, 1, 4, 29_999, 59_999] {
        let r = handle.read_row_blocking(i).unwrap().unwrap();
        let g = i + 1;
        assert_eq!(r[0].as_deref(), Some(g.to_string().as_bytes()), "row {i}");
        assert_eq!(r[3].is_none(), g % 5 == 0, "NULL pattern at row {i}");
    }
    // 退避済みの行を並べ替えて読む
    h.core.send(Command::SortResult {
        result: handle.id(),
        column: 0,
        order: SortOrder::Descending,
    });
    h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.read_row_blocking(0).unwrap().unwrap()[0].as_deref(),
        Some(&b"60000"[..])
    );

    // 結果を閉じるとメモリを手放す
    let id = handle.id();
    h.core.send(Command::CloseResult { result: id });
    h.until(|e| matches!(e, Event::ResultClosed { .. })).await;
    assert!(h.core.result(id).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_with_no_columns_is_handled() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h.exec(tab, "select from generate_series(1, 3)").await;
    let handle = result_handle(&evs);
    assert_eq!((handle.columns().len(), handle.row_count()), (0, 3));
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_a_tab_releases_its_results() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h.exec(tab, "select 1; select 2").await;
    let id = result_handle(&evs).id();
    assert!(h.core.result(id).is_some());
    h.core.send(Command::CloseTab { tab, force: false });
    h.until(|e| matches!(e, Event::TabClosed { .. })).await;
    assert!(h.core.result(id).is_none());
}
