//! 結合テスト用の MySQL / MariaDB サーバー。
//!
//! 接続先は次の順で決まる。
//!
//! 1. 環境変数 `SQLTOOL_TEST_MYSQL_HOST`(と `_PORT`、`_USER`、`_PASSWORD`)があれば、そのサーバーを使う。
//!    Docker の MariaDB や、CI のサービスコンテナ向け。
//! 2. なければ、PATH や Homebrew などから見つけた `mysqld` で、一時ディレクトリに使い捨てのサーバーを
//!    起動する(Unix のみ)。テストプロセスが終了すると、見張りのプロセスが停止して削除する。
//!
//! どちらの場合も、テストごとに専用のデータベースを作って返す。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, OptsBuilder};

#[derive(Debug, Clone)]
pub struct TestDb {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
}

#[derive(Clone)]
struct Server {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
}

static SERVER: OnceLock<Result<Server, String>> = OnceLock::new();
static NEXT_DB: AtomicU32 = AtomicU32::new(0);

/// このテスト専用の空のデータベースを作る。サーバーを用意できなければ `Err`。
pub fn create_database() -> Result<TestDb, String> {
    let server = SERVER
        .get_or_init(start_server)
        .as_ref()
        .map_err(Clone::clone)?;
    let dbname = format!(
        "t{}_{}",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    );
    admin(server, &format!("CREATE DATABASE `{dbname}`"))?;
    Ok(TestDb {
        host: server.host.clone(),
        port: server.port,
        user: server.user.clone(),
        password: server.password.clone(),
        dbname,
    })
}

/// DB を用意できないときにテストを飛ばすための入口。環境変数
/// `SQLTOOL_REQUIRE_MYSQL=1` なら、飛ばさずに失敗させる(CI で見落とさないため)。
pub fn database_or_skip() -> Option<TestDb> {
    match create_database() {
        Ok(db) => Some(db),
        Err(e) if std::env::var_os("SQLTOOL_REQUIRE_MYSQL").is_some() => {
            panic!("MySQL が必要です: {e}")
        }
        Err(e) => {
            eprintln!("SKIP: MySQL を用意できません: {e}");
            None
        }
    }
}

/// 管理用の接続で 1 文を実行する。呼び出し元が非同期テストでも動くよう、専用のスレッドで行う。
fn admin(server: &Server, sql: &str) -> Result<(), String> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<(), String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                rt.block_on(async {
                    let opts = OptsBuilder::default()
                        .ip_or_hostname(server.host.clone())
                        .tcp_port(server.port)
                        .user(Some(server.user.clone()))
                        .pass(server.password.clone())
                        .prefer_socket(false);
                    let mut conn = Conn::new(opts).await.map_err(|e| e.to_string())?;
                    conn.query_drop(sql).await.map_err(|e| e.to_string())?;
                    let _ = conn.disconnect().await;
                    Ok(())
                })
            })
            .join()
            .map_err(|_| "データベース作成のスレッドが panic しました".to_string())?
    })
}

fn start_server() -> Result<Server, String> {
    if let Ok(host) = std::env::var("SQLTOOL_TEST_MYSQL_HOST") {
        return Ok(Server {
            host,
            port: std::env::var("SQLTOOL_TEST_MYSQL_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3306),
            user: std::env::var("SQLTOOL_TEST_MYSQL_USER").unwrap_or_else(|_| "root".into()),
            password: std::env::var("SQLTOOL_TEST_MYSQL_PASSWORD").ok(),
        });
    }
    local::start()
}

#[cfg(not(unix))]
mod local {
    pub fn start() -> Result<super::Server, String> {
        Err("Windows では SQLTOOL_TEST_MYSQL_HOST で接続先を指定してください".into())
    }
}

#[cfg(unix)]
mod local;

/// `10^digits` 行の連番 `g`(0 始まり)を作る式。再帰 CTE は深さの上限があり、MariaDB とも書き方が
/// 違うため、桁ごとの直積で作る。`FROM` の後ろにそのまま置ける(別名は `nums`)。
pub fn numbers(digits: u32) -> String {
    numbers_as(digits, "nums")
}

/// [`numbers`] の別名つき。同じ式を何度も結合するときに使う。
pub fn numbers_as(digits: u32, alias: &str) -> String {
    let d = "(SELECT 0 d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
              UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9)";
    let sum: Vec<String> = (0..digits)
        .map(|i| format!("t{i}.d * {}", 10u64.pow(i)))
        .collect();
    let from: Vec<String> = (0..digits).map(|i| format!("{d} t{i}")).collect();
    format!(
        "(SELECT {} AS g FROM {}) {alias}",
        sum.join(" + "),
        from.join(", ")
    )
}
