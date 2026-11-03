//! 同じ core で Oracle を動かす。
//! 接続先は環境変数(`SQLTOOL_TEST_ORACLE_HOST` など)で指定する。用意できなければ飛ばす
//! (`SQLTOOL_REQUIRE_ORACLE=1` なら失敗させる)。

use sqltool_core::*;

use crate::common::*;

macro_rules! ora {
    () => {
        match test_oracle::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

pub(super) struct Ora {
    pub h: H,
    pub tab: TabId,
    pub db: test_oracle::TestDb,
    pub dir: tempfile::TempDir,
}

impl Ora {
    pub async fn start(db: test_oracle::TestDb, env: EnvLabel) -> Self {
        let mut h = H::start_without_pg().await;
        let tab = h
            .open_net_tab(
                "oracle",
                NetConn {
                    host: &db.host,
                    port: db.port,
                    user: &db.user,
                    password: Some(&db.password),
                    database: &db.service,
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

    /// Oracle では、スキーマは利用者。引用符なしで作った表の名前は、大文字になる。
    pub fn table(&self, name: &str) -> TableRef {
        TableRef::new(&self.db.user, name)
    }

    pub async fn rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let evs = self.h.exec(self.tab, sql).await;
        rows(&result_handle(&evs))
    }
}

mod browsing;
mod editing;
mod plan;
mod statements;
