//! 実行: イベントの順序、複数文、エラー位置、実行対象(カーソル・選択)。

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn select_streams_a_result_and_reports_the_event_sequence() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h
        .exec(tab, "SELECT 1 AS n, NULL::text AS nul, ''::text AS empty, '日本語' AS jp, '\\xdeadbeef'::bytea AS bin")
        .await;

    // 順序: 実行中 → 開始 → 文の開始 → 結果 → 文の完了 → 実行の完了 → 待機
    let tags: Vec<&str> = evs
        .iter()
        .map(|e| match e {
            Event::RunStateChanged {
                state: RunState::Running,
                ..
            } => "running",
            Event::ExecutionStarted { .. } => "started",
            Event::StatementStarted { .. } => "stmt",
            Event::ResultOpened { .. } => "result",
            Event::StatementFinished { .. } => "stmt-done",
            Event::ExecutionFinished { .. } => "done",
            _ => "other",
        })
        .collect();
    assert_eq!(
        tags,
        ["running", "started", "stmt", "result", "stmt-done", "done"]
    );

    let r = result_handle(&evs);
    assert_eq!(
        r.columns()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["n", "nul", "empty", "jp", "bin"]
    );
    assert_eq!(r.columns()[0].class, ValueClass::Number);
    assert!(r.is_finished() && !r.is_truncated() && r.can_sort());
    let got = r.read_row_blocking(0).unwrap().unwrap();
    assert_eq!(got[0].as_deref(), Some(&b"1"[..]));
    assert_eq!(got[1], None, "NULL");
    assert_eq!(got[2].as_deref(), Some(&b""[..]), "empty string");
    assert_eq!(got[3].as_deref(), Some("日本語".as_bytes()));
    assert_eq!(got[4].as_deref(), Some(&[0xde, 0xad, 0xbe, 0xef][..]));
    // UI 向けの読み出し
    let ui = r.with_row(0, |row| row.cell(3).map(<[u8]>::to_vec));
    assert_eq!(ui, Access::Ready(Some("日本語".as_bytes().to_vec())));
    assert!(core_result_is_registered(&h.core, &r));

    let outs = outcomes(&evs);
    let [
        StatementOutcome::Completed {
            rows,
            has_result_set,
            ..
        },
    ] = outs.as_slice()
    else {
        panic!()
    };
    assert_eq!((*rows, *has_result_set), (1, true));
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}

fn core_result_is_registered(core: &Core, r: &ResultHandle) -> bool {
    core.result(r.id()).is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn statements_run_in_order_and_stop_at_the_first_error() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(tab, "create table t (id int)").await;
    let sql = "insert into t values (1); select 1/0; insert into t values (2)";
    let evs = h.exec(tab, sql).await;
    let outs = outcomes(&evs);
    assert_eq!(outs.len(), 2, "the third statement must not run");
    assert!(is_completed(&outs[0]));
    let StatementOutcome::Failed { error, .. } = &outs[1] else {
        panic!("{outs:?}")
    };
    assert_eq!(error.code.as_deref(), Some("22012"));
    assert_eq!(status(&evs), ExecutionStatus::Failed { statement_index: 1 });

    let evs = h.exec(tab, "select count(*) from t").await;
    assert_eq!(
        rows(&result_handle(&evs)),
        [[cell("1")]],
        "only the first insert took effect"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_select_gets_its_own_result() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h
        .exec(tab, "select 1; create table t (x int); select 2, 3")
        .await;
    let handles: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            Event::ResultOpened { result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(handles.len(), 2, "DDL opens no result");
    assert_eq!(handles[0].statement_index(), 0);
    assert_eq!(handles[1].statement_index(), 2);
    assert_ne!(handles[0].id(), handles[1].id());
    assert_eq!(rows(&handles[1]), [[cell("2"), cell("3")]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn error_position_is_translated_to_the_editor_offset() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    // 2 文目の、多バイト文字を含む行にエラーがある
    let sql = "select 1;\n  -- 日本語のコメント\n  select '日本語', from t";
    let evs = h.exec(tab, sql).await;
    let StatementOutcome::Failed {
        editor_offset,
        error,
        ..
    } = outcomes(&evs).pop().unwrap()
    else {
        panic!()
    };
    assert_eq!(error.code.as_deref(), Some("42601"));
    let off = editor_offset.expect("offset");
    assert_eq!(
        &sql[off..off + 4],
        "from",
        "points at the culprit in the whole editor text"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cursor_and_selection_targets() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let sql = "select 'first';\nselect 'second';\nselect 'third'";

    let at = sql.find("second").unwrap() + 2;
    let evs = h
        .exec_with(
            tab,
            sql,
            ExecuteTarget::Cursor { offset: at },
            ExecuteOptions::default(),
        )
        .await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("second")]]);
    assert_eq!(outcomes(&evs).len(), 1);

    let start = sql.find("select 'second'").unwrap();
    let evs = h
        .exec_with(
            tab,
            sql,
            ExecuteTarget::Selection {
                start,
                end: sql.len(),
            },
            ExecuteOptions::default(),
        )
        .await;
    assert_eq!(outcomes(&evs).len(), 2);

    let evs = h
        .exec_with(
            tab,
            "  -- nothing\n",
            ExecuteTarget::All,
            ExecuteOptions::default(),
        )
        .await;
    assert_eq!(status(&evs), ExecutionStatus::NothingToRun);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_execute_on_a_busy_tab_is_rejected() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(
        tab,
        "select pg_sleep(1)",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    h.send_exec(
        tab,
        "select 2",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h.until(|e| matches!(e, Event::CommandFailed { .. })).await;
    let Event::CommandFailed { command, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(*command, "Execute");
    h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
}
