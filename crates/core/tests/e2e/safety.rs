//! 安全機構: 読み取り専用、確認、トランザクションの追跡。

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_connections_start_read_only_and_refuse_writes_entirely() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Production).await;
    h.core.send(Command::OpenTab {
        connection_id: conn.id.clone(),
    });
    let evs = h.until(|e| matches!(e, Event::TabOpened { .. })).await;
    let Event::TabOpened {
        tab,
        read_only,
        connection,
        ..
    } = evs.last().unwrap().clone()
    else {
        unreachable!()
    };
    assert!(read_only, "production starts read-only");
    assert_eq!(connection.env_label, EnvLabel::Production);

    // 書き込みを含むなら、前の SELECT も含めて何も実行しない
    let evs = h.exec(tab, "select 1; create table t (id int)").await;
    assert_eq!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly {
            statement_index: 1,
            editor_offset: 10
        }
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, Event::StatementStarted { .. }))
    );

    // DB 側の設定も効いている(sqlscan をすり抜けても止まる)
    h.core.send(Command::SetReadOnly {
        tab,
        read_only: false,
    });
    h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    h.exec(tab, "create table t (id int)").await;
    h.core.send(Command::SetReadOnly {
        tab,
        read_only: true,
    });
    h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: true,
                ..
            }
        )
    })
    .await;
    // DB 側が止める文も、sqlscan が先に拒否する
    let evs = h
        .exec(
            tab,
            "select set_config('default_transaction_read_only', 'off', false)",
        )
        .await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    let evs = h.exec(tab, "insert into t values (1)").await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    let evs = h.exec(tab, "select * from t").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_is_enforced_by_the_database_even_when_the_scanner_is_bypassed() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Production).await;
    // sqlscan が読み取りと見なす文でも、DB 側が書き込みを拒否する(nextval は書き込み)
    h.core.send(Command::SetReadOnly {
        tab,
        read_only: false,
    });
    h.until(|e| matches!(e, Event::ReadOnlyChanged { .. }))
        .await;
    h.exec(tab, "create sequence s").await;
    h.core.send(Command::SetReadOnly {
        tab,
        read_only: true,
    });
    h.until(|e| matches!(e, Event::ReadOnlyChanged { .. }))
        .await;
    let evs = h.exec(tab, "select nextval('s')").await;
    let StatementOutcome::Failed { error, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    assert_eq!(error.code.as_deref(), Some("25006"), "{error:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unfiltered_delete_and_drop_need_confirmation() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(
        tab,
        "create table t (id int); insert into t values (1), (2)",
    )
    .await;

    // 断ると、何も実行されない
    h.send_exec(
        tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h
        .until(|e| matches!(e, Event::ConfirmationRequired { .. }))
        .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ConfirmKind::UnfilteredDelete);
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::RunStateChanged {
            state: RunState::WaitingConfirmation,
            ..
        }
    )));
    h.core.send(Command::Confirm { id, accept: false });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(status(&evs), ExecutionStatus::Declined);
    let evs = h.exec(tab, "select count(*) from t").await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("2")]]);

    // 承諾すると実行される
    h.send_exec(
        tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h
        .until(|e| matches!(e, Event::ConfirmationRequired { .. }))
        .await;
    let Event::ConfirmationRequired { id, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    h.core.send(Command::Confirm { id, accept: true });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    let outs = outcomes(&evs);
    let [StatementOutcome::Completed { affected_rows, .. }] = outs.as_slice() else {
        panic!("{evs:?}")
    };
    assert_eq!(*affected_rows, Some(2));

    // DROP は対象名を添えて確認する。WHERE 付きの DELETE は確認しない
    h.exec(tab, "delete from t where id = 1").await;
    h.send_exec(
        tab,
        "drop table  t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h
        .until(|e| matches!(e, Event::ConfirmationRequired { .. }))
        .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items[0].kind, ConfirmKind::Drop);
    assert_eq!(items[0].summary, "drop table t");
    // 確認の待ち中に Cancel すると、断ったことになる
    h.core.send(Command::Cancel { tab });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(status(&evs), ExecutionStatus::Declined);
    let _ = id;
    let evs = h.exec(tab, "select count(*) from t").await;
    assert_eq!(
        status(&evs),
        ExecutionStatus::Completed,
        "table still exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_flagged_statement_in_a_batch_is_listed_in_one_prompt() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(
        tab,
        "create table a (x int); truncate a; update a set x = 1; select 1",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs = h
        .until(|e| matches!(e, Event::ConfirmationRequired { .. }))
        .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(
        items
            .iter()
            .map(|i| (i.statement_index, i.kind))
            .collect::<Vec<_>>(),
        [
            (1, ConfirmKind::Truncate),
            (2, ConfirmKind::UnfilteredUpdate)
        ]
    );
    assert!(
        !h.log
            .iter()
            .any(|e| matches!(e, Event::StatementStarted { .. })),
        "nothing runs before the answer"
    );
    h.core.send(Command::Confirm { id, accept: true });
    let evs = h
        .until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(outcomes(&evs).len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transaction_state_is_tracked_and_guards_closing_the_tab() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    assert!(h.core.pending_transactions().is_empty());

    let evs = h.exec(tab, "begin").await;
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::TransactionChanged {
            state: TxnState::Open,
            ..
        }
    )));
    assert_eq!(h.core.pending_transactions(), [(tab, TxnState::Open)]);

    // 閉じようとすると止められ、強制すれば閉じる
    h.core.send(Command::CloseTab { tab, force: false });
    let evs = h.until(|e| matches!(e, Event::CloseBlocked { .. })).await;
    assert!(matches!(
        evs.last().unwrap(),
        Event::CloseBlocked {
            reason: CloseBlock::PendingTransaction(TxnState::Open),
            ..
        }
    ));

    // エラーで中断すると Failed になり、ROLLBACK で戻る
    let evs = h.exec(tab, "select 1/0").await;
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::TransactionChanged {
            state: TxnState::Failed,
            ..
        }
    )));
    let evs = h.exec(tab, "select 1").await;
    let StatementOutcome::Failed { error, .. } = outcomes(&evs).pop().unwrap() else {
        panic!()
    };
    assert_eq!(error.code.as_deref(), Some("25P02"));
    let evs = h.exec(tab, "rollback").await;
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::TransactionChanged {
            state: TxnState::Idle,
            ..
        }
    )));
    assert!(h.core.pending_transactions().is_empty());

    h.exec(tab, "begin").await;
    h.core.send(Command::CloseTab { tab, force: true });
    h.until(|e| matches!(e, Event::TabClosed { .. })).await;
    assert!(h.core.pending_transactions().is_empty());
}
