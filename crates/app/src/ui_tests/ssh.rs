//! SSH トンネル経由の接続: 初めてのサーバーは、指紋の確認を経て接続する。

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use serde_json::json;
use sqltool_core::{ConnectionInput, ConnectionOptions, EnvLabel, PasswordChange, TlsSetting};
use test_ssh::Sshd;

use super::harness::*;
use crate::model::Retry;

fn click(cx: &mut TestAppContext, f: &Fixture, id: &'static str) {
    with_window(cx, f, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
}

#[gpui_kit::test]
fn an_unknown_ssh_host_key_is_confirmed_on_screen_and_then_the_tab_opens(cx: &mut TestAppContext) {
    let Some(sshd) = Sshd::start() else { return };
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);

    // 同じ DB へ、SSH トンネル経由で接続する設定を足す
    let (db_port, db_name, db_user, db_host) = cx.update(|cx| {
        let c = f.model.read(cx).connections[0].clone();
        (c.port, c.database, c.username, c.host)
    });
    let mut options = ConnectionOptions {
        tls: TlsSetting::Disable,
        ..Default::default()
    };
    for (k, v) in [
        ("ssh", json!(true)),
        ("ssh_host", json!("127.0.0.1")),
        ("ssh_port", json!(sshd.port)),
        ("ssh_user", json!(Sshd::user_name())),
        ("ssh_auth", json!("key")),
        (
            "ssh_key_file",
            json!(sshd.path("user").display().to_string()),
        ),
    ] {
        options.extra.insert(k.into(), v);
    }
    cx.update(|cx| {
        f.model.update(cx, |m, _| {
            m.save_connection(ConnectionInput {
                id: None,
                name: "via ssh".into(),
                driver: "postgresql".into(),
                host: db_host,
                port: db_port,
                database: db_name,
                username: db_user,
                options,
                env_label: EnvLabel::Dev,
                password: PasswordChange::Keep,
                ssh_secret: PasswordChange::Keep,
            })
        })
    });
    wait_until(cx, "the connection to be listed", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .connections
                .iter()
                .any(|c| c.name == "via ssh")
        })
    });
    let id = cx.update(|cx| {
        f.model
            .read(cx)
            .connections
            .iter()
            .find(|c| c.name == "via ssh")
            .unwrap()
            .id
            .clone()
    });

    // 開こうとすると、エラーでなく、指紋の確認が出る
    cx.update(|cx| f.model.update(cx, |m, _| m.open_tab(id.clone())));
    wait_until(cx, "the host key prompt", |cx| {
        cx.update(|cx| f.model.read(cx).host_key.is_some())
    });
    cx.update(|cx| {
        let m = f.model.read(cx);
        let p = m.host_key.as_ref().unwrap();
        assert_eq!(p.fingerprint, sshd.host_fingerprint());
        assert_eq!(p.retry, Some(Retry::Open(id.clone())));
        assert_eq!(m.tabs.len(), 1, "no tab until the key is trusted");
        assert!(
            m.notice
                .as_ref()
                .is_none_or(|n| n.level != crate::model::Level::Error),
            "a prompt is not an error: {:?}",
            m.notice.as_ref().map(|n| &n.text)
        );
    });

    // 信頼すると、接続して、タブが開く
    click(cx, &f, "host-key-trust");
    wait_until(cx, "the second tab", |cx| {
        cx.update(|cx| f.model.read(cx).tabs.len() == 2)
    });
    cx.update(|cx| {
        let m = f.model.read(cx);
        assert!(m.host_key.is_none());
        assert_eq!(m.tabs[1].connection.name, "via ssh");
    });
    run_sql(cx, &f, "select 1");
}
