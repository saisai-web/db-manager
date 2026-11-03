//! 実行計画(MySQL / MariaDB)と、SSH トンネル経由の接続。

use sqltool_core::*;

use super::My;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_returns_a_plan_tree_on_mysql_and_mariadb() {
    let mut m = My::start(my!(), EnvLabel::Dev).await;
    m.h.exec(m.tab, "create table big (id int primary key, v varchar(10)); insert into big values (1,'a'),(2,'b'),(3,'c')").await;
    let explain = async |m: &mut My, sql: &str, analyze: bool| {
        m.h.core.send(Command::Explain {
            tab: m.tab,
            sql: sql.into(),
            target: ExecuteTarget::All,
            analyze,
        });
        m.h.until(|e| {
            matches!(
                e,
                Event::ExplainReady { .. }
                    | Event::CommandFailed {
                        command: "Explain",
                        ..
                    }
            )
        })
        .await
        .pop()
        .unwrap()
    };

    // 実行せずに、見積もりの木を求める
    let ev = explain(&mut m, "select * from big where v = 'b'", false).await;
    let Event::ExplainReady { plan, analyze, .. } = ev else {
        panic!("explain failed: {ev:?}")
    };
    assert!(!analyze);
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    assert!(
        all.iter()
            .any(|n| n.target.as_deref().is_some_and(|t| t.starts_with("big"))),
        "{all:#?}"
    );
    assert_eq!(plan.execution_ms, None, "no timings without analyze");

    // 実測つき: 実際に実行して、時間と行数が付く
    let ev = explain(&mut m, "select * from big where id > 0", true).await;
    let Event::ExplainReady { plan, .. } = ev else {
        panic!("analyze failed: {ev:?}")
    };
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    assert!(
        plan.execution_ms.is_some() && all.iter().any(|n| n.actual_rows.is_some()),
        "{plan:#?}"
    );

    // 書き込みを含む文には、実測を使わせない
    let ev = explain(&mut m, "delete from big where id = 1", true).await;
    assert!(matches!(ev, Event::CommandFailed { .. }), "{ev:?}");
    assert_eq!(m.rows("select count(*) from big").await[0][0], cell("3"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mysql_connects_through_an_ssh_tunnel_after_the_host_key_is_trusted() {
    use serde_json::json;
    let db = my!();
    let Some(sshd) = test_ssh::Sshd::start() else {
        return;
    };
    let mut h = H::start_without_pg().await;
    let mut input = h.input("my via ssh", EnvLabel::Dev);
    input.driver = "mysql".into();
    input.host = Some(db.host.clone());
    input.port = Some(db.port);
    input.database = Some(db.dbname.clone());
    input.username = Some(db.user.clone());
    input.password = match &db.password {
        Some(p) => PasswordChange::Set(Secret::new(p.clone())),
        None => PasswordChange::Keep,
    };
    let x = &mut input.options.extra;
    x.insert("ssh".into(), json!(true));
    x.insert("ssh_host".into(), json!("127.0.0.1"));
    x.insert("ssh_port".into(), json!(sshd.port));
    x.insert("ssh_user".into(), json!(test_ssh::Sshd::user_name()));
    x.insert("ssh_auth".into(), json!("key"));
    x.insert(
        "ssh_key_file".into(),
        json!(sshd.path("user").display().to_string()),
    );
    h.core.send(Command::SaveConnection(input));
    let Some(Event::ConnectionSaved(conn)) = h
        .until(|e| matches!(e, Event::ConnectionSaved(_) | Event::CommandFailed { .. }))
        .await
        .pop()
    else {
        panic!("not saved")
    };

    // 信頼していないホスト鍵では、DB に届く前に止まる(トンネルを通っている証拠)
    h.core.send(Command::OpenTab {
        connection_id: conn.id.clone(),
    });
    let evs = h
        .until(|e| matches!(e, Event::TabOpened { .. } | Event::CommandFailed { .. }))
        .await;
    let Some(Event::CommandFailed { error, .. }) = evs.last().cloned() else {
        panic!("{evs:?}")
    };
    assert_eq!(error.code.as_deref(), Some(HOST_KEY_UNKNOWN));

    h.core.send(Command::TrustSshHost {
        host: "127.0.0.1".into(),
        port: sshd.port,
        fingerprint: sshd.host_fingerprint(),
    });
    let tab = h.open_tab_for(&conn.id).await;
    let evs = h.exec(tab, "select database(), 1 + 1").await;
    assert_eq!(rows(&result_handle(&evs)), [[cell(&db.dbname), cell("2")]]);
    // スキーマの取得(メタデータ用の別接続)も、同じトンネルを通る
    h.core.send(Command::LoadSchemas {
        connection_id: conn.id.clone(),
    });
    let evs = h
        .until(|e| matches!(e, Event::Schemas { .. } | Event::CommandFailed { .. }))
        .await;
    assert!(
        matches!(evs.last().unwrap(), Event::Schemas { .. }),
        "{evs:?}"
    );
}
