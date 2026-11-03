//! DB サーバーなしで確かめられる core の振る舞い。

use std::sync::Arc;
use std::time::Duration;

use sqltool_core::*;
use storage::MemorySecretStore;
use tokio::runtime::Handle;

fn input(name: &str, driver: &str) -> ConnectionInput {
    ConnectionInput {
        id: None,
        name: name.into(),
        driver: driver.into(),
        host: Some("db.example".into()),
        port: Some(5432),
        database: Some("app".into()),
        username: Some("alice".into()),
        options: ConnectionOptions::default(),
        env_label: EnvLabel::Production,
        password: PasswordChange::Set(Secret::new("offline-test-password")),
        ssh_secret: PasswordChange::Keep,
    }
}

async fn next(ev: &mut EventStream) -> Event {
    tokio::time::timeout(Duration::from_secs(10), ev.recv())
        .await
        .expect("event")
        .expect("open")
}

/// ドライバを使わないので、名前だけ登録しておく。
struct NoDriver;

#[async_trait::async_trait]
impl driver_api::Driver for NoDriver {
    fn capabilities(&self) -> &driver_api::Capabilities {
        unreachable!("not used")
    }
    fn dialect(&self) -> &dyn driver_api::Dialect {
        unreachable!("not used")
    }
    async fn connect(
        &self,
        _: &driver_api::ConnectionConfig,
    ) -> driver_api::Result<Box<dyn driver_api::Session>> {
        Err(driver_api::DriverError::new(
            driver_api::ErrorKind::Connect,
            "offline",
        ))
    }
    async fn connect_metadata(
        &self,
        _: &driver_api::ConnectionConfig,
    ) -> driver_api::Result<Box<dyn driver_api::MetadataProvider>> {
        Err(driver_api::DriverError::new(
            driver_api::ErrorKind::Connect,
            "offline",
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_connections_survive_a_restart_and_the_file_holds_no_password() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("data/app.db");
    let secrets = Arc::new(MemorySecretStore::new());
    let make = |secrets: Arc<MemorySecretStore>| {
        let drivers = DriverRegistry::new().with("postgresql", Arc::new(NoDriver));
        let mut cfg = CoreConfig::new(secrets, drivers);
        cfg.database_path = Some(db_path.clone());
        Core::start(cfg, Handle::current()).unwrap()
    };

    let saved = {
        let (core, mut ev) = make(secrets.clone());
        core.send(Command::SaveConnection(input("prod", "postgresql")));
        match next(&mut ev).await {
            Event::ConnectionSaved(c) => c,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(saved.env_label, EnvLabel::Production);
    assert!(saved.has_password);

    // 再起動
    let (core, mut ev) = make(secrets.clone());
    core.send(Command::ListConnections);
    let Event::Connections(list) = next(&mut ev).await else {
        panic!()
    };
    assert_eq!(list, [saved]);

    for entry in std::fs::read_dir(db_path.parent().unwrap()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        let needle = b"offline-test-password";
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "password found on disk"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_failures_are_reported_as_command_failures() {
    let drivers = DriverRegistry::new().with("postgresql", Arc::new(NoDriver));
    let (core, mut ev) = Core::start(
        CoreConfig::new(Arc::new(MemorySecretStore::new()), drivers),
        Handle::current(),
    )
    .unwrap();
    core.send(Command::SaveConnection(input("x", "postgresql")));
    let Event::ConnectionSaved(c) = next(&mut ev).await else {
        panic!()
    };

    core.send(Command::OpenTab {
        connection_id: c.id.clone(),
    });
    match next(&mut ev).await {
        Event::CommandFailed {
            command: "OpenTab",
            error,
            ..
        } => {
            assert_eq!(error.kind, ErrorKind::Connect);
            assert!(!format!("{error:?}").contains("offline-test-password"));
        }
        other => panic!("{other:?}"),
    }
    // 存在しない接続、存在しないタブ
    core.send(Command::OpenTab {
        connection_id: "nope".into(),
    });
    assert!(matches!(
        next(&mut ev).await,
        Event::CommandFailed {
            command: "OpenTab",
            ..
        }
    ));
    core.send(Command::Execute {
        tab: TabId(999),
        sql: "select 1".into(),
        target: ExecuteTarget::All,
        options: ExecuteOptions::default(),
    });
    assert!(matches!(
        next(&mut ev).await,
        Event::CommandFailed {
            command: "Execute",
            tab: Some(TabId(999)),
            ..
        }
    ));
    core.send(Command::CloseResult {
        result: ResultId(5),
    });
    assert!(
        matches!(next(&mut ev).await, Event::ResultClosed { .. }),
        "closing an unknown result is harmless"
    );
    core.send(Command::FetchMore {
        result: ResultId(5),
        more: FetchMore::Rest,
    });
    assert!(matches!(
        next(&mut ev).await,
        Event::CommandFailed {
            command: "FetchMore",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_output_of_commands_never_shows_the_password() {
    let cmd = Command::SaveConnection(input("x", "postgresql"));
    assert!(!format!("{cmd:?}").contains("offline-test-password"));
    assert!(!format!("{:#?}", cmd).contains("offline-test-password"));
}
