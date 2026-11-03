//! 画面テストの共通基盤。

use std::sync::Arc;
use std::time::{Duration, Instant};

use driver_mysql::MysqlDriver;
use driver_postgres::PostgresDriver;
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use sqltool_core::{
    Command, ConnectionInput, ConnectionOptions, Core, CoreConfig, DriverRegistry, EnvLabel,
    PasswordChange, Secret, TlsSetting,
};

use crate::launch::Launch;
use crate::model::AppModel;
use crate::ui::Workspace;

pub struct Fixture {
    /// DB への通信と結果の受信を行うランタイム。テストの間、生かしておく
    pub _runtime: tokio::runtime::Runtime,
    pub model: Entity<AppModel>,
    pub window: WindowHandle<Root>,
    /// テスト用 DB のパスワード。無ければ(認証なしの使い捨てサーバー)`None`
    pub password: Option<String>,
}

/// テスト用 DB のパスワードを、接続の保存に渡す形にする。
pub fn password_change(password: &Option<String>) -> PasswordChange {
    password.clone().map_or(PasswordChange::Keep, |p| {
        PasswordChange::Set(Secret::new(p))
    })
}

/// 接続を 1 つ登録して開いた画面を返す。DB を用意できなければ `None`。
pub fn open(cx: &mut TestAppContext, production: bool) -> Option<Fixture> {
    let db = test_pg::database_or_skip()?;
    // DB の応答は、Tokio のスレッドから来る。テスト用の決定論的な実行器に、それを許す
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let drivers = DriverRegistry::new()
        .with("postgresql", Arc::new(PostgresDriver))
        .with("mysql", Arc::new(MysqlDriver))
        .with("sqlserver", Arc::new(driver_sqlserver::SqlServerDriver))
        .with("oracle", Arc::new(driver_oracle::OracleDriver));
    let config = CoreConfig::new(Arc::new(storage::MemorySecretStore::new()), drivers);
    let (core, events): (Core, _) = Core::start(config, runtime.handle().clone()).unwrap();
    let env_label = if production {
        EnvLabel::Production
    } else {
        EnvLabel::Dev
    };
    core.send(Command::SaveConnection(ConnectionInput {
        id: None,
        name: "test".into(),
        driver: "postgresql".into(),
        host: Some(db.host.clone()),
        port: Some(db.port),
        database: Some(db.dbname.clone()),
        username: Some(db.user.clone()),
        options: ConnectionOptions {
            tls: TlsSetting::Disable,
            ..Default::default()
        },
        env_label,
        password: password_change(&db.password),
        ssh_secret: PasswordChange::Keep,
    }));

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::actions::bind_keys(cx);
    });
    let mut model = None;
    let window = cx.update(|cx| {
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1280.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let m = AppModel::start(core, events, window.window_handle(), cx);
                m.update(cx, |m, _| {
                    m.set_launch(Launch {
                        import: Some("pending".into()),
                        open: Some("test".into()),
                        run: None,
                        table: None,
                        explain: None,
                        er: None,
                    })
                });
                model = Some(m.clone());
                cx.new(|cx| Workspace::new(m, cx))
            },
        )
        .expect("open test window");
        window.downcast::<Root>().expect("Root")
    });
    Some(Fixture {
        _runtime: runtime,
        model: model.unwrap(),
        window,
        password: db.password,
    })
}

/// 条件が成り立つまで、イベントを処理しながら待つ。DB の応答は別スレッドから来るため、
/// 少しずつ眠りながら繰り返す。
pub fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut cond: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if cond(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// 最初のタブが開くのを待つ。
pub fn wait_for_tab(cx: &mut TestAppContext, f: &Fixture) {
    wait_until(cx, "the tab to open", |cx| {
        cx.update(|cx| f.model.read(cx).active_tab().is_some())
    });
}

/// ウィンドウの中で処理する(クリックやキー入力、入力欄への値の設定には、ウィンドウが要る)。
pub fn with_window<R>(
    cx: &mut TestAppContext,
    f: &Fixture,
    body: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
) -> R {
    use gpui_kit::AppContext as _;
    cx.update_window(f.window.into(), |_, window, cx| body(window, cx))
        .unwrap()
}

/// タブのエディタに SQL を入れて、全文を実行し、終わるまで待つ。
pub fn run_sql(cx: &mut TestAppContext, f: &Fixture, sql: &str) {
    use gpui_kit::component::input::InputContent;
    with_window(cx, f, |window, cx| {
        let editor = f.model.read(cx).active_tab().unwrap().editor.clone();
        editor.update(cx, |e, cx| {
            e.set_value(InputContent::from(sql.to_string()), window, cx)
        });
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, cx| m.run(tab, true, cx));
    });
    wait_until(cx, "the statements to finish", |cx| {
        cx.update(|cx| {
            let t = f.model.read(cx).active_tab().unwrap();
            t.is_idle() && !t.status.text.starts_with("実行中")
        })
    });
}
