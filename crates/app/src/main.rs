//! SQLツールの画面(GPUI Kit)。画面の状態と描画だけを持ち、DB に関わる処理はすべて core に任せる。

mod actions;
mod launch;
mod model;
mod ui;
#[cfg(test)]
mod ui_tests;

use std::sync::Arc;

use driver_mysql::MysqlDriver;
use driver_oracle::OracleDriver;
use driver_postgres::PostgresDriver;
use driver_sqlite::SqliteDriver;
use driver_sqlserver::SqlServerDriver;
use gpui_kit::component::Root;
use gpui_kit::*;
use model::AppModel;
use sqltool_core::{Core, CoreConfig, DriverRegistry};

fn main() {
    let launch = match launch::Launch::parse(std::env::args().skip(1)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    // DB への通信と結果の受信は Tokio のランタイムで動かす(画面のスレッドでは行わない)
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sqltool-rt")
        .build()
        .expect("ランタイムを起動できません");
    let drivers = DriverRegistry::new()
        .with("postgresql", Arc::new(PostgresDriver))
        .with("mysql", Arc::new(MysqlDriver))
        .with("sqlserver", Arc::new(SqlServerDriver))
        .with("oracle", Arc::new(OracleDriver))
        .with("sqlite", Arc::new(SqliteDriver));
    let config = CoreConfig::desktop(drivers).expect("設定を初期化できません");
    let (core, events) =
        Core::start(config, runtime.handle().clone()).expect("core を起動できません");
    if let Some(path) = &launch.import {
        match std::fs::read_to_string(path) {
            Ok(json) => core.send(sqltool_core::Command::ImportConnections { json }),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        actions::bind_keys(cx);
        let window = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(800.)),
                    cx,
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let model = AppModel::start(core, events, window.window_handle(), cx);
                model.update(cx, |m, _| m.set_launch(launch));
                let view = cx.new(|cx| ui::Workspace::new(model, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("ウィンドウを開けません");
        let _ = window;
        cx.activate(true);
    });
    drop(runtime);
}
