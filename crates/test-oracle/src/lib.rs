//! 結合テスト用の Oracle サーバー。
//!
//! 接続先は環境変数で指定する。Oracle は手元で使い捨てのサーバーを起動できない(Docker の
//! `gvenzl/oracle-free` などを別に用意する)ので、環境変数がなければ、テストは飛ばす。
//!
//! * `SQLTOOL_TEST_ORACLE_HOST`(必須)、`_PORT`(既定 1521)、`_SERVICE`(既定 `FREEPDB1`)
//! * `SQLTOOL_TEST_ORACLE_USER`(既定 `system`)、`_PASSWORD`(管理用の接続)
//!
//! テストごとに、専用の利用者(= スキーマ)を作って返す。Oracle には「データベースを作る」操作が無い。

mod tls_proxy;

pub use tls_proxy::TlsProxy;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// 作る利用者のパスワード(使い捨てのテスト用)
pub const USER_PASSWORD: &str = "Sqltool_t1_pw";
/// 古いテスト用の利用者を消すときの目安。他のテストプロセスが使っている利用者は消さない
const STALE_AFTER_HOURS: u32 = 2;
const PREFIX: &str = "SQLTOOL_T";

#[derive(Debug, Clone)]
pub struct TestDb {
    pub host: String,
    pub port: u16,
    /// このテスト専用の利用者(= スキーマ名)
    pub user: String,
    pub password: String,
    pub service: String,
}

#[derive(Clone)]
struct Server {
    host: String,
    port: u16,
    service: String,
    admin_user: String,
    admin_password: String,
}

static SERVER: OnceLock<Result<Server, String>> = OnceLock::new();
static NEXT_USER: AtomicU32 = AtomicU32::new(0);

fn server() -> Result<&'static Server, String> {
    SERVER
        .get_or_init(|| {
            let host = std::env::var("SQLTOOL_TEST_ORACLE_HOST")
                .map_err(|_| "SQLTOOL_TEST_ORACLE_HOST で接続先を指定してください".to_string())?;
            let s = Server {
                host,
                port: std::env::var("SQLTOOL_TEST_ORACLE_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(1521),
                service: std::env::var("SQLTOOL_TEST_ORACLE_SERVICE")
                    .unwrap_or_else(|_| "FREEPDB1".into()),
                admin_user: std::env::var("SQLTOOL_TEST_ORACLE_USER")
                    .unwrap_or_else(|_| "system".into()),
                admin_password: std::env::var("SQLTOOL_TEST_ORACLE_PASSWORD").unwrap_or_default(),
            };
            // 以前のテストが残した利用者を片づける(失敗しても、テストは続ける)
            let _ = drop_stale_users(&s);
            Ok(s)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn admin_connection(s: &Server) -> Result<oracledb::Connection, String> {
    let config = oracledb::Config::default()
        .set_credentials(&s.admin_user, &s.admin_password)
        .set_connect_string(&format!("{}:{}/{}", s.host, s.port, s.service))
        .map_err(|e| e.to_string())?;
    oracledb::connect(config).map_err(|e| e.to_string())
}

fn drop_stale_users(s: &Server) -> Result<(), String> {
    let conn = admin_connection(s)?;
    let cursor = conn
        .query(
            &format!(
                "SELECT username FROM dba_users WHERE username LIKE '{PREFIX}%' \
                 AND created < SYSDATE - {STALE_AFTER_HOURS}/24"
            ),
            &[],
        )
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = cursor
        .filter_map(Result::ok)
        .filter_map(|r| r.get::<String>(0).ok())
        .collect();
    for name in names {
        let _ = conn.execute(&format!("DROP USER \"{name}\" CASCADE"), &[]);
    }
    Ok(())
}

/// このテスト専用の利用者(空のスキーマ)を作る。サーバーを用意できなければ `Err`。
pub fn create_database() -> Result<TestDb, String> {
    let s = server()?;
    let user = format!(
        "{PREFIX}{}_{}",
        std::process::id(),
        NEXT_USER.fetch_add(1, Ordering::Relaxed)
    );
    let conn = admin_connection(s)?;
    for sql in [
        format!(
            "CREATE USER \"{user}\" IDENTIFIED BY \"{USER_PASSWORD}\" QUOTA UNLIMITED ON USERS"
        ),
        format!(
            "GRANT CREATE SESSION, CREATE TABLE, CREATE VIEW, CREATE SEQUENCE, \
             CREATE PROCEDURE, CREATE TRIGGER TO \"{user}\""
        ),
        // 実測の実行計画が読む V$ 表のため
        format!("GRANT SELECT_CATALOG_ROLE TO \"{user}\""),
    ] {
        conn.execute(&sql, &[]).map_err(|e| format!("{sql}: {e}"))?;
    }
    Ok(TestDb {
        host: s.host.clone(),
        port: s.port,
        user,
        password: USER_PASSWORD.into(),
        service: s.service.clone(),
    })
}

/// DB を用意できないときにテストを飛ばすための入口。環境変数
/// `SQLTOOL_REQUIRE_ORACLE=1` なら、飛ばさずに失敗させる(CI で見落とさないため)。
pub fn database_or_skip() -> Option<TestDb> {
    match create_database() {
        Ok(db) => Some(db),
        Err(e) if std::env::var_os("SQLTOOL_REQUIRE_ORACLE").is_some() => {
            panic!("Oracle が必要です: {e}")
        }
        Err(e) => {
            eprintln!("SKIP: Oracle を用意できません: {e}");
            None
        }
    }
}

/// 管理用の接続で、権限が要る操作(他の利用者の `GRANT` など)を行う。
pub fn admin(sql: &str) -> Result<(), String> {
    let conn = admin_connection(server()?)?;
    conn.execute(sql, &[])
        .map(|_| ())
        .map_err(|e| e.to_string())
}
