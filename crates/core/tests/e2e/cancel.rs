//! キャンセルとタイムアウト。

use std::time::{Duration, Instant};

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_returns_control_quickly_and_the_tab_stays_usable() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(
        tab,
        "select pg_sleep(60)",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    h.core.send(Command::Cancel { tab });
    let evs = h
        .until(|e| {
            matches!(
                e,
                Event::RunStateChanged {
                    state: RunState::Cancelling,
                    ..
                }
            )
        })
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "UI is told within a second: {:?}",
        started.elapsed()
    );
    assert!(!evs.is_empty());

    // 他のコマンドは、停止の完了を待たずに処理される
    h.core.send(Command::ListConnections);
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(evs.iter().any(|e| matches!(e, Event::Connections(_))));
    assert!(matches!(
        outcomes(&evs).as_slice(),
        [StatementOutcome::Cancelled { .. }]
    ));
    assert_eq!(status(&evs), ExecutionStatus::Cancelled);

    let evs = h.exec(tab, "select 'ok'").await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("ok")]]);
}

/// 実行の直後に停止しても、失われずに止まる(要求がクエリより先にサーバーへ着いても)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_sent_immediately_after_execute_is_not_lost() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    for delay_ms in [0u64, 1, 3, 10] {
        let started = Instant::now();
        h.send_exec(
            tab,
            "select pg_sleep(60)",
            ExecuteTarget::All,
            ExecuteOptions::default(),
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        h.core.send(Command::Cancel { tab });
        let evs = h
            .until(|e| matches!(e, Event::ExecutionFinished { .. }))
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "delay {delay_ms}ms: took {:?}",
            started.elapsed()
        );
        assert!(
            matches!(status(&evs), ExecutionStatus::Cancelled),
            "delay {delay_ms}ms: {:?}",
            status(&evs)
        );
    }
    let evs = h.exec(tab, "select 'still fine'").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_the_rest_of_the_batch() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(
        tab,
        "select pg_sleep(60); select 2",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    h.core.send(Command::Cancel { tab });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(outcomes(&evs).len(), 1, "the second statement must not run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_asks_the_server_to_stop_and_reports_it() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let started = Instant::now();
    let evs = h
        .exec_with(
            tab,
            "select pg_sleep(60)",
            ExecuteTarget::All,
            ExecuteOptions {
                timeout: Some(Duration::from_millis(300)),
                ..Default::default()
            },
        )
        .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        matches!(outcomes(&evs).as_slice(), [StatementOutcome::TimedOut { limit, .. }] if *limit == Duration::from_millis(300))
    );
    assert_eq!(
        status(&evs),
        ExecutionStatus::TimedOut { statement_index: 0 }
    );
    let evs = h.exec(tab, "select 1").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}
