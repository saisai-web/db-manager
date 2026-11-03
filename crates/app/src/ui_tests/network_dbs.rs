//! ネットワークで接続する DB(MySQL / MariaDB、SQL Server、Oracle): 接続フォームの種類の切り替えと、画面からの接続と実行。

use gpui_kit::AppContext as _;
use gpui_kit::TestAppContext;
use sqltool_core::{ConnectionInput, ConnectionOptions, EnvLabel, PasswordChange, TlsSetting};

use super::harness::*;
use crate::ui::{ConnectionForm, KINDS};

#[gpui_kit::test]
fn switching_the_kind_in_the_form_changes_the_default_port_only_while_it_is_untouched(
    cx: &mut TestAppContext,
) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    let kind = |id: &str| KINDS.iter().find(|k| k.id == id).unwrap();
    with_window(cx, &f, |window, cx| {
        let model = f.model.clone();
        let form = cx.new(|cx| ConnectionForm::new(model, None, window, cx));
        assert_eq!(form.read(cx).driver_id(), "postgresql");
        assert_eq!(form.read(cx).port_text(cx), "5432");

        form.update(cx, |form, cx| form.select_driver(kind("mysql"), window, cx));
        assert_eq!(form.read(cx).driver_id(), "mysql");
        assert_eq!(
            form.read(cx).port_text(cx),
            "3306",
            "the default follows the kind"
        );

        form.update(cx, |form, cx| {
            form.select_driver(kind("sqlite"), window, cx)
        });
        assert_eq!(form.read(cx).port_text(cx), "", "a file has no port");

        form.update(cx, |form, cx| form.select_driver(kind("mysql"), window, cx));
        assert_eq!(form.read(cx).port_text(cx), "3306");
    });
}

/// 接続先の情報。
struct Target {
    driver: &'static str,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: Option<String>,
}

/// 接続を保存して開き、SQL を実行して、1 行 2 列の結果を確かめる。
fn connects_and_runs(cx: &mut TestAppContext, t: Target, sql: &str, expect: (&str, &str)) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    cx.update(|cx| {
        f.model.update(cx, |m, _| {
            m.save_connection(ConnectionInput {
                id: None,
                name: "net".into(),
                driver: t.driver.into(),
                host: Some(t.host.clone()),
                port: Some(t.port),
                database: Some(t.database.clone()),
                username: Some(t.user.clone()),
                options: ConnectionOptions {
                    tls: TlsSetting::Disable,
                    ..Default::default()
                },
                env_label: EnvLabel::Dev,
                password: t.password.clone().map_or(PasswordChange::Keep, |p| {
                    PasswordChange::Set(sqltool_core::Secret::new(p))
                }),
                ssh_secret: PasswordChange::Keep,
            })
        })
    });
    wait_until(cx, "the connection to be listed", |cx| {
        cx.update(|cx| f.model.read(cx).connections.iter().any(|c| c.name == "net"))
    });
    let id = cx.update(|cx| {
        f.model
            .read(cx)
            .connections
            .iter()
            .find(|c| c.name == "net")
            .unwrap()
            .id
            .clone()
    });
    cx.update(|cx| f.model.update(cx, |m, _| m.open_tab(id)));
    wait_until(cx, "the new tab", |cx| {
        cx.update(|cx| f.model.read(cx).tabs.len() == 2)
    });
    let second = cx.update(|cx| f.model.read(cx).tabs[1].id);
    cx.update(|cx| f.model.update(cx, |m, cx| m.activate(second, cx)));

    run_sql(cx, &f, sql);
    cx.update(|cx| {
        let t = f.model.read(cx).active_tab().unwrap();
        assert_eq!(t.results.len(), 1, "{:?}", t.status.text);
        let r = &t.results[0].handle;
        assert_eq!(r.row_count(), 1);
        let row = r.read_row_blocking(0).unwrap().unwrap();
        assert_eq!(row[0].as_deref(), Some(expect.0.as_bytes()));
        assert_eq!(row[1].as_deref(), Some(expect.1.as_bytes()));
    });
}

#[gpui_kit::test]
fn a_mysql_connection_opens_and_runs_a_query_from_the_screen(cx: &mut TestAppContext) {
    let Some(db) = test_mysql::database_or_skip() else {
        return;
    };
    let t = Target {
        driver: "mysql",
        host: db.host,
        port: db.port,
        database: db.dbname,
        user: db.user,
        password: db.password,
    };
    connects_and_runs(
        cx,
        t,
        "select '日本語' as jp, 1.50 as price",
        ("日本語", "1.50"),
    );
}

#[gpui_kit::test]
fn a_sql_server_connection_opens_and_runs_a_query_from_the_screen(cx: &mut TestAppContext) {
    let Some(db) = test_sqlserver::database_or_skip() else {
        return;
    };
    let t = Target {
        driver: "sqlserver",
        host: db.host,
        port: db.port,
        database: db.dbname,
        user: db.user,
        password: db.password,
    };
    connects_and_runs(
        cx,
        t,
        "select N'日本語' as jp, cast(1.5 as decimal(5,2)) as price",
        ("日本語", "1.50"),
    );
}

#[gpui_kit::test]
fn an_oracle_connection_opens_and_runs_a_query_from_the_screen(cx: &mut TestAppContext) {
    let Some(db) = test_oracle::database_or_skip() else {
        return;
    };
    let t = Target {
        driver: "oracle",
        host: db.host,
        port: db.port,
        // Oracle では、データベース欄がサービス名
        database: db.service,
        user: db.user,
        password: Some(db.password),
    };
    connects_and_runs(
        cx,
        t,
        "select '日本語' as jp, cast(1.5 as number(5,2)) as price from dual",
        ("日本語", "1.5"),
    );
}
