//! 接続設定: パスワードの保管、保存・更新・削除、接続テスト。

use std::time::{Duration, Instant};

use sqltool_core::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_passwords_live_only_in_the_secret_store() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Dev).await;
    assert!(conn.has_password);
    assert_eq!(h.secrets.len(), 1);

    h.core.send(Command::ListConnections);
    let evs = h.until(|e| matches!(e, Event::Connections(_))).await;
    let Event::Connections(list) = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(list.len(), 1);

    h.core.send(Command::ExportConnections);
    let evs = h
        .until(|e| matches!(e, Event::ConnectionsExported(_)))
        .await;
    let Event::ConnectionsExported(json) = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(!json.contains(PASSWORD));

    // 接続して使っても、パスワードはイベントのどこにも現れない
    let tab = h.open_tab_for(&conn.id).await;
    h.exec(tab, "select 1").await;
    let everything = format!("{:?}", h.log);
    assert!(
        !everything.contains(PASSWORD),
        "password leaked into events"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updating_and_clearing_a_password() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Dev).await;
    let key = format!("connection:{}", conn.id);
    use storage::SecretStore;
    assert_eq!(h.secrets.get(&key).unwrap().as_deref(), Some(PASSWORD));

    let mut input = h.input("renamed", EnvLabel::Staging);
    input.id = Some(conn.id.clone());
    input.password = PasswordChange::Set(Secret::new("second"));
    h.core.send(Command::SaveConnection(input.clone()));
    let evs = h.until(|e| matches!(e, Event::ConnectionSaved(_))).await;
    let Event::ConnectionSaved(updated) = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(updated.id, conn.id);
    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.env_label, EnvLabel::Staging);
    assert_eq!(h.secrets.get(&key).unwrap().as_deref(), Some("second"));
    assert_eq!(h.secrets.len(), 1, "no duplicate secret");

    input.password = PasswordChange::Keep;
    h.core.send(Command::SaveConnection(input.clone()));
    h.until(|e| matches!(e, Event::ConnectionSaved(_))).await;
    assert_eq!(h.secrets.get(&key).unwrap().as_deref(), Some("second"));

    input.password = PasswordChange::Clear;
    h.core.send(Command::SaveConnection(input));
    let evs = h.until(|e| matches!(e, Event::ConnectionSaved(_))).await;
    let Event::ConnectionSaved(cleared) = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(!cleared.has_password);
    assert_eq!(h.secrets.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_connection_reports_the_version_or_a_clean_error() {
    let db = pg!();
    let mut h = H::start(db).await;
    let ok = h.input("t", EnvLabel::Dev);
    h.core.send(Command::TestConnection(ok.clone()));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(Ok(version)) = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert!(version.starts_with("PostgreSQL"));

    let mut bad = ok;
    bad.port = Some(1);
    h.core.send(Command::TestConnection(bad));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(Err(err)) = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert_eq!(err.kind, ErrorKind::Connect);
    assert!(!format!("{err:?}").contains(PASSWORD));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unregistered_driver_and_blank_name_are_rejected() {
    let db = pg!();
    let mut h = H::start(db).await;
    let mut input = h.input("x", EnvLabel::Dev);
    input.driver = "db2".into();
    h.core.send(Command::SaveConnection(input));
    let evs = h.until(|e| matches!(e, Event::CommandFailed { .. })).await;
    let Event::CommandFailed { command, error, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(*command, "SaveConnection");
    assert_eq!(error.kind, ErrorKind::Unsupported);

    let blank = h.input("   ", EnvLabel::Dev);
    h.core.send(Command::SaveConnection(blank));
    h.until(|e| matches!(e, Event::CommandFailed { .. })).await;
    assert!(
        h.secrets.is_empty(),
        "nothing is stored for a rejected connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_connection_removes_its_secret_and_needs_its_tabs_closed() {
    let db = pg!();
    let mut h = H::start(db).await;
    let conn = h.save(EnvLabel::Dev).await;
    let tab = h.open_tab_for(&conn.id).await;

    h.core.send(Command::DeleteConnection {
        id: conn.id.clone(),
    });
    h.until(|e| matches!(e, Event::CommandFailed { .. })).await;
    assert_eq!(h.secrets.len(), 1);

    h.core.send(Command::CloseTab { tab, force: false });
    h.until(|e| matches!(e, Event::TabClosed { .. })).await;
    h.core.send(Command::DeleteConnection {
        id: conn.id.clone(),
    });
    h.until(|e| matches!(e, Event::ConnectionDeleted { .. }))
        .await;
    // パスワードの削除はイベントの後に行われる
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.secrets.is_empty() {
        assert!(Instant::now() < deadline, "secret was not deleted");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_settings_reach_the_driver_including_the_ca_file() {
    let db = pg!();
    let Some(ca) = db.ssl_cert.clone() else {
        eprintln!("SKIP: TLS 対応のテスト用サーバーがありません");
        return;
    };
    let mut h = H::start(db).await;

    // 証明書を検証する(既定)。CA を指定しなければ、接続できず、理由が分かる
    let mut input = h.input("tls", EnvLabel::Dev);
    input.options.tls = TlsSetting::Verify;
    h.core.send(Command::TestConnection(input.clone()));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(Err(err)) = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert_eq!(err.kind, ErrorKind::Connect);
    assert!(
        err.message.to_lowercase().contains("certificate"),
        "{}",
        err.message
    );

    // CA を指定すると、検証つきで接続できる
    input.options.extra.insert(
        "tls_ca_file".into(),
        serde_json::Value::String(ca.display().to_string()),
    );
    h.core.send(Command::TestConnection(input.clone()));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    assert!(
        matches!(evs.last().unwrap(), Event::ConnectionTested(Ok(_))),
        "{evs:?}"
    );

    // 保存した接続でタブを開いて、実際に暗号化されている
    input.id = None;
    h.core.send(Command::SaveConnection(input));
    let evs = h.until(|e| matches!(e, Event::ConnectionSaved(_))).await;
    let Event::ConnectionSaved(conn) = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert!(
        conn.options.extra.contains_key("tls_ca_file"),
        "the setting survives saving"
    );
    let tab = h.open_tab_for(&conn.id).await;
    let evs = h
        .exec(
            tab,
            "select ssl from pg_stat_ssl where pid = pg_backend_pid()",
        )
        .await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("t")]]);
}
