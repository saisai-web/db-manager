//! 同じ core で MySQL / MariaDB を動かす。
//! サーバーを用意できなければ飛ばす(`SQLTOOL_REQUIRE_MYSQL=1` なら失敗させる)。

use sqltool_core::*;

use crate::common::*;

macro_rules! my {
    () => {
        match test_mysql::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

pub(super) struct My {
    pub h: H,
    pub tab: TabId,
    pub db: test_mysql::TestDb,
    pub dir: tempfile::TempDir,
}

impl My {
    pub async fn start(db: test_mysql::TestDb, env: EnvLabel) -> Self {
        let mut h = H::start_without_pg().await;
        let tab = h
            .open_net_tab(
                "mysql",
                NetConn {
                    host: &db.host,
                    port: db.port,
                    user: &db.user,
                    password: db.password.as_deref(),
                    database: &db.dbname,
                },
                env,
            )
            .await;
        Self {
            h,
            tab,
            db,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn table(&self, name: &str) -> TableRef {
        TableRef::new(&self.db.dbname, name)
    }

    pub async fn rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let evs = self.h.exec(self.tab, sql).await;
        rows(&result_handle(&evs))
    }

    pub async fn set_read_only(&mut self, read_only: bool) {
        self.h.core.send(Command::SetReadOnly {
            tab: self.tab,
            read_only,
        });
        self.h
            .until(|e| matches!(e, Event::ReadOnlyChanged { read_only: r, .. } if *r == read_only))
            .await;
    }
}

mod browsing;
mod editing;
mod plan_and_ssh;
mod statements;
