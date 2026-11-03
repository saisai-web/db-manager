//! SSH トンネル経由の接続。使い捨ての sshd(127.0.0.1、鍵認証)を挟んで、実際の PostgreSQL に接続する。
//! sshd を用意できない環境では飛ばす。

use serde_json::json;
use sqltool_core::*;
use storage::SecretStore;
use test_ssh::{LOCKED_KEY_PASSPHRASE, Sshd};

use crate::common::*;

fn ssh_input(h: &H, sshd: &Sshd, key: &str) -> ConnectionInput {
    let mut input = h.input("via ssh", EnvLabel::Dev);
    let x = &mut input.options.extra;
    x.insert("ssh".into(), json!(true));
    x.insert("ssh_host".into(), json!("127.0.0.1"));
    x.insert("ssh_port".into(), json!(sshd.port));
    x.insert("ssh_user".into(), json!(Sshd::user_name()));
    x.insert("ssh_auth".into(), json!("key"));
    x.insert(
        "ssh_key_file".into(),
        json!(sshd.path(key).display().to_string()),
    );
    input
}

async fn save(h: &mut H, input: ConnectionInput) -> ConnectionSummary {
    h.core.send(Command::SaveConnection(input));
    match h
        .until(|e| matches!(e, Event::ConnectionSaved(_) | Event::CommandFailed { .. }))
        .await
        .pop()
        .unwrap()
    {
        Event::ConnectionSaved(c) => c,
        other => panic!("{other:?}"),
    }
}

/// タブを開こうとして、失敗したときのイベントを返す。
async fn open_fails(h: &mut H, id: &str) -> (Option<Event>, UserError) {
    h.core.send(Command::OpenTab {
        connection_id: id.into(),
    });
    let evs = h
        .until(|e| matches!(e, Event::TabOpened { .. } | Event::CommandFailed { .. }))
        .await;
    match evs.last().unwrap().clone() {
        Event::CommandFailed { error, .. } => {
            let prompt = evs
                .into_iter()
                .find(|e| matches!(e, Event::SshHostKeyUnconfirmed { .. }));
            (prompt, error)
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_host_key_needs_confirmation_then_the_connection_works() {
    let db = pg!();
    let Some(sshd) = Sshd::start() else { return };
    let mut h = H::start(db).await;
    let input = ssh_input(&h, &sshd, "user");
    let conn = save(&mut h, input).await;

    // 初めてのサーバー: 指紋を見せて確認を求める(パスワードも鍵も、確かめる前には使わない)
    let (prompt, error) = open_fails(&mut h, &conn.id).await;
    assert_eq!(error.code.as_deref(), Some(HOST_KEY_UNKNOWN));
    let Some(Event::SshHostKeyUnconfirmed {
        host,
        port,
        fingerprint,
    }) = prompt
    else {
        panic!("no prompt event")
    };
    assert_eq!((host.as_str(), port), ("127.0.0.1", sshd.port));
    assert_eq!(fingerprint, sshd.host_fingerprint());
    assert_eq!(error.detail.as_deref(), Some(fingerprint.as_str()));

    // 信頼して、やり直す
    h.core.send(Command::TrustSshHost {
        host,
        port,
        fingerprint,
    });
    let tab = h.open_tab_for(&conn.id).await;
    let evs = h.exec(tab, "select current_database(), 1 + 1").await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::ExecutionFinished { .. })),
        "{evs:?}"
    );
    // スキーマの取得(メタデータ用の別接続)も、同じトンネルを通る
    h.core.send(Command::LoadSchemas {
        connection_id: conn.id.clone(),
    });
    let evs = h
        .until(|e| matches!(e, Event::Schemas { .. } | Event::CommandFailed { .. }))
        .await;
    let last = evs.last().unwrap();
    assert!(matches!(last, Event::Schemas { .. }), "{last:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_host_key_is_refused_until_it_is_forgotten() {
    let db = pg!();
    let Some(sshd) = Sshd::start() else { return };
    let mut h = H::start(db).await;
    let input = ssh_input(&h, &sshd, "user");
    let conn = save(&mut h, input).await;
    // 別の鍵を信頼済みとして記録しておく(サーバーが入れ替わった状況)
    h.core.send(Command::TrustSshHost {
        host: "127.0.0.1".into(),
        port: sshd.port,
        fingerprint: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
    });
    let (prompt, error) = open_fails(&mut h, &conn.id).await;
    assert!(prompt.is_none(), "a changed key must not offer to trust it");
    assert_eq!(error.code.as_deref(), Some(HOST_KEY_CHANGED));
    assert!(error.hint.is_some());

    // 忘れると、未登録に戻る(確認からやり直せる)
    h.core.send(Command::ForgetSshHost {
        host: "127.0.0.1".into(),
        port: sshd.port,
    });
    h.until(|e| matches!(e, Event::SshHostKeyForgotten)).await;
    let (_, error) = open_fails(&mut h, &conn.id).await;
    assert_eq!(error.code.as_deref(), Some(HOST_KEY_UNKNOWN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ssh_passphrase_lives_only_in_the_secret_store_and_is_used() {
    let db = pg!();
    let Some(sshd) = Sshd::start() else { return };
    let mut h = H::start(db).await;
    let mut input = ssh_input(&h, &sshd, "locked");
    input.ssh_secret = PasswordChange::Set(Secret::new(LOCKED_KEY_PASSPHRASE));
    let conn = save(&mut h, input.clone()).await;
    let key = format!("ssh:{}", conn.id);
    assert_eq!(
        h.secrets.get(&key).unwrap().as_deref(),
        Some(LOCKED_KEY_PASSPHRASE)
    );
    assert_eq!(conn.options.extra.get("ssh_secret"), Some(&json!(true)));

    // 設定のどこにも、秘密は出ない
    h.core.send(Command::ExportConnections);
    let evs = h
        .until(|e| matches!(e, Event::ConnectionsExported(_)))
        .await;
    let Event::ConnectionsExported(json) = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(!json.contains(LOCKED_KEY_PASSPHRASE));

    h.core.send(Command::TrustSshHost {
        host: "127.0.0.1".into(),
        port: sshd.port,
        fingerprint: sshd.host_fingerprint(),
    });
    let tab = h.open_tab_for(&conn.id).await;
    h.exec(tab, "select 1").await;
    let everything = format!("{:?}", h.log);
    assert!(
        !everything.contains(LOCKED_KEY_PASSPHRASE),
        "passphrase leaked"
    );

    // 保存し直して(秘密は Keep)も、印と秘密は残る。SSH をやめると、秘密も消える
    input.id = Some(conn.id.clone());
    input.ssh_secret = PasswordChange::Keep;
    let kept = save(&mut h, input.clone()).await;
    assert_eq!(kept.options.extra.get("ssh_secret"), Some(&json!(true)));
    assert!(h.secrets.get(&key).unwrap().is_some());

    h.core.send(Command::CloseTab { tab, force: true });
    h.until(|e| matches!(e, Event::TabClosed { .. })).await;
    input.options.extra.insert("ssh".into(), json!(false));
    let off = save(&mut h, input).await;
    assert_eq!(off.options.extra.get("ssh_secret"), None);
    assert!(
        h.secrets.get(&key).unwrap().is_none(),
        "unused secret removed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn testing_a_connection_goes_through_the_tunnel_and_a_wrong_key_is_an_auth_error() {
    let db = pg!();
    let Some(sshd) = Sshd::start() else { return };
    let mut h = H::start(db).await;
    h.core.send(Command::TrustSshHost {
        host: "127.0.0.1".into(),
        port: sshd.port,
        fingerprint: sshd.host_fingerprint(),
    });
    h.core
        .send(Command::TestConnection(ssh_input(&h, &sshd, "user")));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(r) = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(r.is_ok(), "{r:?}");

    test_ssh::keygen(&sshd.path("stranger"), "");
    h.core
        .send(Command::TestConnection(ssh_input(&h, &sshd, "stranger")));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(Err(e)) = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert_eq!(e.kind, ErrorKind::Auth);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn using_ssh_without_a_port_or_host_is_a_clear_error() {
    let db = pg!();
    let Some(sshd) = Sshd::start() else { return };
    let mut h = H::start(db).await;
    let mut input = ssh_input(&h, &sshd, "user");
    input.options.extra.remove("ssh_host");
    h.core.send(Command::TestConnection(input));
    let evs = h.until(|e| matches!(e, Event::ConnectionTested(_))).await;
    let Event::ConnectionTested(Err(e)) = evs.last().unwrap() else {
        panic!("{evs:?}")
    };
    assert!(e.message.contains("SSH"), "{}", e.message);
}
