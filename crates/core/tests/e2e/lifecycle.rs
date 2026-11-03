//! 切断、再接続、終了。

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_connection_is_reported_and_reconnect_restores_the_tab() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(tab, "begin").await;
    assert_eq!(h.core.pending_transactions().len(), 1);

    let evs = h
        .exec(tab, "select pg_terminate_backend(pg_backend_pid())")
        .await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::TabDisconnected { .. })),
        "{evs:#?}"
    );
    assert!(
        h.core.pending_transactions().is_empty(),
        "the transaction died with the connection"
    );
    assert!(
        matches!(outcomes(&evs).as_slice(), [StatementOutcome::Failed { error, .. }] if error.kind == ErrorKind::Disconnected)
    );

    // 切断中の実行は、失敗として知らせる
    h.send_exec(
        tab,
        "select 1",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    h.until(|e| {
        matches!(
            e,
            Event::CommandFailed {
                command: "Execute",
                ..
            }
        )
    })
    .await;
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(status(&evs), ExecutionStatus::Failed { statement_index: 0 });

    h.core.send(Command::Reconnect { tab });
    h.until(|e| matches!(e, Event::TabOpened { .. })).await;
    let evs = h.exec(tab, "select 'back'").await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("back")]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_closes_everything() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    let evs = h.exec(tab, "select 1").await;
    let id = result_handle(&evs).id();
    h.core.send(Command::Shutdown);
    h.until(|e| matches!(e, Event::ShutdownComplete)).await;
    assert!(h.core.result(id).is_none());
    h.core.send(Command::Execute {
        tab,
        sql: "select 1".into(),
        target: ExecuteTarget::All,
        options: Default::default(),
    });
    h.until(|e| matches!(e, Event::CommandFailed { .. })).await;
}
