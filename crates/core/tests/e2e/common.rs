//! 結合テストの共通基盤。UI と同じ「コマンドを送り、イベントを受ける」形で core を動かす。
use std::sync::Arc;
use std::time::Duration;

use driver_postgres::PostgresDriver;
use sqltool_core::*;
use storage::MemorySecretStore;
use tokio::runtime::Handle;

pub const PASSWORD: &str = "pw-never-to-be-leaked-9f3a";

/// ネットワークで接続する DB の接続先。
pub struct NetConn<'a> {
    pub host: &'a str,
    pub port: u16,
    pub user: &'a str,
    pub password: Option<&'a str>,
    pub database: &'a str,
}

pub struct H {
    pub core: Core,
    pub ev: EventStream,
    pub log: Vec<Event>,
    pub db: test_pg::TestDb,
    pub secrets: Arc<MemorySecretStore>,
}

impl H {
    pub async fn start(db: test_pg::TestDb) -> H {
        Self::start_with(db, |_| {}).await
    }

    pub async fn start_with(db: test_pg::TestDb, tweak: impl FnOnce(&mut CoreConfig)) -> H {
        let secrets = Arc::new(MemorySecretStore::new());
        let drivers = DriverRegistry::new()
            .with("postgresql", Arc::new(PostgresDriver))
            .with("sqlite", Arc::new(driver_sqlite::SqliteDriver))
            .with("mysql", Arc::new(driver_mysql::MysqlDriver))
            .with("oracle", Arc::new(driver_oracle::OracleDriver))
            .with("sqlserver", Arc::new(driver_sqlserver::SqlServerDriver));
        let mut cfg = CoreConfig::new(secrets.clone(), drivers);
        tweak(&mut cfg);
        let (core, ev) = Core::start(cfg, Handle::current()).unwrap();
        H {
            core,
            ev,
            log: Vec::new(),
            db,
            secrets,
        }
    }

    /// PostgreSQL なしで、SQLite だけを使う(接続先の `db` は、使わないので空)。
    pub async fn start_sqlite() -> H {
        Self::start_without_pg().await
    }

    /// PostgreSQL を使わない(MySQL など、ほかの DB のテスト用)。
    pub async fn start_without_pg() -> H {
        let unused = test_pg::TestDb {
            host: String::new(),
            port: 0,
            user: String::new(),
            password: None,
            dbname: String::new(),
            ssl_cert: None,
        };
        Self::start_with(unused, |_| {}).await
    }

    /// SQLite のファイルへの接続を保存して、タブを開く。
    pub async fn open_sqlite_tab(&mut self, path: &std::path::Path, env: EnvLabel) -> TabId {
        let mut input = self.input("sqlite", env);
        input.driver = "sqlite".into();
        input.host = Some(path.display().to_string());
        input.port = None;
        input.database = None;
        input.username = None;
        input.password = PasswordChange::Keep;
        self.core.send(Command::SaveConnection(input));
        let evs = self
            .until(|e| matches!(e, Event::ConnectionSaved(_) | Event::CommandFailed { .. }))
            .await;
        let Some(Event::ConnectionSaved(conn)) = evs.last().cloned() else {
            panic!("{evs:?}")
        };
        self.open_tab_for(&conn.id).await
    }

    /// ネットワークで接続する DB(PostgreSQL 以外)への接続を保存して、タブを開く。
    pub async fn open_net_tab(&mut self, driver: &str, conn: NetConn<'_>, env: EnvLabel) -> TabId {
        let mut input = self.input(driver, env);
        input.driver = driver.into();
        input.host = Some(conn.host.into());
        input.port = Some(conn.port);
        input.database = Some(conn.database.into());
        input.username = Some(conn.user.into());
        input.password = match conn.password {
            Some(p) => PasswordChange::Set(Secret::new(p)),
            None => PasswordChange::Keep,
        };
        self.core.send(Command::SaveConnection(input));
        let evs = self
            .until(|e| matches!(e, Event::ConnectionSaved(_) | Event::CommandFailed { .. }))
            .await;
        let Some(Event::ConnectionSaved(saved)) = evs.last().cloned() else {
            panic!("{evs:?}")
        };
        self.open_tab_for(&saved.id).await
    }

    /// タブが使っている接続の id。
    pub fn core_connection_id(&self, tab: TabId) -> String {
        self.log
            .iter()
            .find_map(|e| match e {
                Event::TabOpened {
                    tab: t, connection, ..
                } if *t == tab => Some(connection.id.clone()),
                _ => None,
            })
            .expect("the tab was opened")
    }

    pub async fn recv(&mut self) -> Event {
        let e = tokio::time::timeout(Duration::from_secs(30), self.ev.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for an event; log: {:#?}",
                    self.log_tail()
                )
            })
            .expect("event stream closed");
        self.log.push(e.clone());
        e
    }

    pub fn log_tail(&self) -> Vec<String> {
        self.log
            .iter()
            .rev()
            .take(15)
            .map(|e| format!("{e:?}"))
            .collect()
    }

    /// 条件に合うイベントまでを受け取り、そこまでのイベントを返す(一致したものを含む)。
    pub async fn until(&mut self, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut got = Vec::new();
        loop {
            let e = self.recv().await;
            let hit = pred(&e);
            got.push(e);
            if hit {
                return got;
            }
        }
    }

    pub fn input(&self, name: &str, env: EnvLabel) -> ConnectionInput {
        ConnectionInput {
            id: None,
            name: name.into(),
            driver: "postgresql".into(),
            host: Some(self.db.host.clone()),
            port: Some(self.db.port),
            database: Some(self.db.dbname.clone()),
            username: Some(self.db.user.clone()),
            options: ConnectionOptions {
                tls: TlsSetting::Disable,
                ..Default::default()
            },
            env_label: env,
            password: PasswordChange::Set(Secret::new(PASSWORD)),
            ssh_secret: PasswordChange::Keep,
        }
    }

    pub async fn save(&mut self, env: EnvLabel) -> ConnectionSummary {
        let input = self.input("test db", env);
        self.core.send(Command::SaveConnection(input));
        match self
            .until(|e| matches!(e, Event::ConnectionSaved(_) | Event::CommandFailed { .. }))
            .await
            .pop()
            .unwrap()
        {
            Event::ConnectionSaved(c) => c,
            other => panic!("{other:?}"),
        }
    }

    pub async fn open_tab(&mut self, env: EnvLabel) -> TabId {
        let conn = self.save(env).await;
        self.open_tab_for(&conn.id).await
    }

    pub async fn open_tab_for(&mut self, connection_id: &str) -> TabId {
        self.core.send(Command::OpenTab {
            connection_id: connection_id.into(),
        });
        match self
            .until(|e| matches!(e, Event::TabOpened { .. } | Event::CommandFailed { .. }))
            .await
            .pop()
            .unwrap()
        {
            Event::TabOpened { tab, .. } => tab,
            other => panic!("{other:?}"),
        }
    }

    pub fn send_exec(&self, tab: TabId, sql: &str, target: ExecuteTarget, options: ExecuteOptions) {
        self.core.send(Command::Execute {
            tab,
            sql: sql.into(),
            target,
            options,
        });
    }

    /// 全文を実行し、実行が終わるまでのイベントを返す。
    pub async fn exec(&mut self, tab: TabId, sql: &str) -> Vec<Event> {
        self.exec_with(tab, sql, ExecuteTarget::All, ExecuteOptions::default())
            .await
    }

    pub async fn exec_with(
        &mut self,
        tab: TabId,
        sql: &str,
        target: ExecuteTarget,
        options: ExecuteOptions,
    ) -> Vec<Event> {
        self.send_exec(tab, sql, target, options);
        self.until(|e| {
            matches!(e, Event::ExecutionFinished { tab: t, .. } if *t == tab)
                || matches!(e, Event::CommandFailed { .. })
        })
        .await
    }
}

pub fn outcomes(events: &[Event]) -> Vec<StatementOutcome> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::StatementFinished { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}

pub fn status(events: &[Event]) -> ExecutionStatus {
    events
        .iter()
        .find_map(|e| match e {
            Event::ExecutionFinished { status, .. } => Some(*status),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ExecutionFinished in {events:#?}"))
}

pub fn result_handle(events: &[Event]) -> ResultHandle {
    events
        .iter()
        .find_map(|e| match e {
            Event::ResultOpened { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("a result was opened")
}

pub fn rows(h: &ResultHandle) -> Vec<Vec<Option<String>>> {
    (0..h.row_count())
        .map(|i| {
            h.read_row_blocking(i)
                .unwrap()
                .unwrap()
                .into_iter()
                .map(|c| c.map(|b| String::from_utf8(b).unwrap()))
                .collect()
        })
        .collect()
}

pub fn cell(v: &str) -> Option<String> {
    Some(v.to_string())
}

pub fn is_completed(o: &StatementOutcome) -> bool {
    matches!(o, StatementOutcome::Completed { .. })
}
