//! 履歴とスキーマ。

use std::time::{Duration, Instant};

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_records_statements_but_never_password_bearing_ones() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(tab, "select 1; select 1/0").await;
    h.exec(
        tab,
        "create role hist_user login password 'role-secret-xyz'",
    )
    .await;
    h.exec(tab, "alter role hist_user password 'another-secret-xyz'")
        .await;
    h.exec(tab, "select 'visible'").await;

    h.core.send(Command::ListHistory(HistoryQuery {
        limit: 50,
        ..Default::default()
    }));
    let evs = h.until(|e| matches!(e, Event::History(_))).await;
    let Event::History(list) = evs.last().unwrap() else {
        unreachable!()
    };
    let texts: Vec<_> = list.iter().map(|e| e.sql_text.as_str()).collect();
    assert_eq!(
        texts,
        ["select 'visible'", "select 1/0", "select 1"],
        "newest first"
    );
    assert!(texts.iter().all(|t| !t.contains("secret")));
    assert_eq!(list[1].status, storage::HistoryStatus::Error);
    assert_eq!(list[2].status, storage::HistoryStatus::Ok);
    assert_eq!(list[2].affected_rows, Some(1));
    assert!(list[2].duration_ms.is_some());

    h.core.send(Command::ClearHistory {
        connection_id: None,
    });
    h.until(|e| matches!(e, Event::History(_))).await;
    h.core.send(Command::ListHistory(HistoryQuery {
        limit: 50,
        ..Default::default()
    }));
    let evs = h.until(|e| matches!(e, Event::History(_))).await;
    assert!(matches!(evs.last().unwrap(), Event::History(l) if l.is_empty()));
    // イベントにも、パスワードを含む文は現れない
    assert!(!format!("{:?}", h.log).contains("role-secret-xyz"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schema_browsing_and_invalidation_after_ddl() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Dev).await;
    let tab = h.open_tab_for(&conn.id).await;
    h.exec(
        tab,
        "create schema app; create table app.users (id bigint primary key, name text not null)",
    )
    .await;

    let cid = conn.id.clone();
    h.core.send(Command::LoadSchemas {
        connection_id: cid.clone(),
    });
    let evs = h.until(|e| matches!(e, Event::Schemas { .. })).await;
    let Event::Schemas { schemas, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(schemas.iter().any(|s| s.name == "app"));

    h.core.send(Command::LoadTables {
        connection_id: cid.clone(),
        schema: "app".into(),
    });
    let evs = h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["users"]
    );

    h.core.send(Command::LoadTableDetails {
        connection_id: cid.clone(),
        table: TableRef::new("app", "users"),
    });
    let evs = h.until(|e| matches!(e, Event::TableDetails { .. })).await;
    let Event::TableDetails { details, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(details.columns.len(), 2);
    assert!(details.keys.iter().any(|k| k.kind == KeyKind::Primary));
    assert_eq!(details.indexes.len(), 1);

    // 結果はキャッシュされる。DDL を実行すると無効になり、読み直すと新しい表が見える
    let evs = h.exec(tab, "create table app.orders (id int)").await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::SchemaInvalidated { .. }))
    );
    h.core.send(Command::LoadTables {
        connection_id: cid.clone(),
        schema: "app".into(),
    });
    let evs = h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(tables.len(), 2, "cache was invalidated by the DDL");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schema_loads_do_not_wait_for_a_running_query() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Dev).await;
    let tab = h.open_tab_for(&conn.id).await;
    h.send_exec(
        tab,
        "select pg_sleep(3)",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    let started = Instant::now();
    h.core.send(Command::LoadSchemas {
        connection_id: conn.id.clone(),
    });
    h.until(|e| matches!(e, Event::Schemas { .. })).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    h.core.send(Command::Cancel { tab });
    h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snippets_can_be_saved_updated_and_deleted_and_history_can_be_searched() {
    let mut h = H::start_sqlite().await;
    let dir = tempfile::tempdir().unwrap();
    let tab = h
        .open_sqlite_tab(&dir.path().join("t.db"), EnvLabel::Dev)
        .await;

    let snippets = |evs: &[Event]| match evs.last().unwrap() {
        Event::Snippets(l) => l.clone(),
        other => panic!("{other:?}"),
    };
    h.core.send(Command::SaveSnippet {
        id: None,
        name: "  top users ".into(),
        sql: "select * from users limit 10".into(),
    });
    let list = snippets(&h.until(|e| matches!(e, Event::Snippets(_))).await);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "top users", "the name is trimmed");
    let id = list[0].id.clone();

    h.core.send(Command::SaveSnippet {
        id: Some(id.clone()),
        name: "Top users".into(),
        sql: "select 1".into(),
    });
    let list = snippets(&h.until(|e| matches!(e, Event::Snippets(_))).await);
    assert_eq!(
        (list.len(), list[0].sql_text.as_str(), list[0].name.as_str()),
        (1, "select 1", "Top users")
    );

    h.core.send(Command::SaveSnippet {
        id: None,
        name: "   ".into(),
        sql: "x".into(),
    });
    h.until(|e| {
        matches!(
            e,
            Event::CommandFailed {
                command: "SaveSnippet",
                ..
            }
        )
    })
    .await;

    h.core.send(Command::DeleteSnippet { id });
    assert!(snippets(&h.until(|e| matches!(e, Event::Snippets(_))).await).is_empty());

    // 履歴は、含まれる文字列で絞り込める
    h.exec(tab, "create table t (a int)").await;
    h.exec(tab, "select 'needle-in-haystack'").await;
    h.exec(tab, "select 2").await;
    h.core.send(Command::ListHistory(HistoryQuery {
        contains: Some("needle".into()),
        limit: 10,
        ..Default::default()
    }));
    let evs = h.until(|e| matches!(e, Event::History(_))).await;
    let Event::History(found) = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(found.len(), 1);
    assert!(found[0].sql_text.contains("needle"));
}
