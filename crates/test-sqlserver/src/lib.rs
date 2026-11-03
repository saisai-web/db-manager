//! 結合テスト用の SQL Server サーバー。
//!
//! SQL Server は手元で使い捨てに起動できないため(Linux 版を Docker などで動かす)、接続先は環境変数
//! `SQLTOOL_TEST_MSSQL_HOST`(と `_PORT`、`_USER`、`_PASSWORD`)で指定する。無ければ、テストは飛ばす。
//! テストごとに専用のデータベースを作って返す。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

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

/// このテスト専用の空のデータベースを作る。サーバーを指定されていなければ `Err`。
pub fn create_database() -> Result<TestDb, String> {
    let server = SERVER
        .get_or_init(find_server)
        .as_ref()
        .map_err(Clone::clone)?;
    let dbname = format!(
        "t{}_{}",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    );
    admin(server, &format!("CREATE DATABASE [{dbname}]"))?;
    Ok(TestDb {
        host: server.host.clone(),
        port: server.port,
        user: server.user.clone(),
        password: server.password.clone(),
        dbname,
    })
}

/// DB を用意できないときにテストを飛ばすための入口。環境変数
/// `SQLTOOL_REQUIRE_MSSQL=1` なら、飛ばさずに失敗させる(CI で見落とさないため)。
pub fn database_or_skip() -> Option<TestDb> {
    match create_database() {
        Ok(db) => Some(db),
        Err(e) if std::env::var_os("SQLTOOL_REQUIRE_MSSQL").is_some() => {
            panic!("SQL Server が必要です: {e}")
        }
        Err(e) => {
            eprintln!("SKIP: SQL Server を用意できません: {e}");
            None
        }
    }
}

fn find_server() -> Result<Server, String> {
    let host = std::env::var("SQLTOOL_TEST_MSSQL_HOST")
        .map_err(|_| "SQLTOOL_TEST_MSSQL_HOST で接続先を指定してください".to_string())?;
    let server = Server {
        host,
        port: std::env::var("SQLTOOL_TEST_MSSQL_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(1433),
        user: std::env::var("SQLTOOL_TEST_MSSQL_USER").unwrap_or_else(|_| "sa".into()),
        password: std::env::var("SQLTOOL_TEST_MSSQL_PASSWORD").ok(),
    };
    // 前の実行が残したデータベースを片づける(1 時間以上前に作られた、このテストの名前のもの)
    let _ = admin(
        &server,
        "DECLARE @n sysname, @sql nvarchar(400); \\
         DECLARE c CURSOR FOR SELECT name FROM sys.databases \\
           WHERE name LIKE 't[0-9]%[_][0-9]%' AND create_date < DATEADD(hour, -1, SYSUTCDATETIME()); \\
         OPEN c; FETCH NEXT FROM c INTO @n; \\
         WHILE @@FETCH_STATUS = 0 BEGIN \\
           SET @sql = N'ALTER DATABASE [' + @n + N'] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [' + @n + N']'; \\
           BEGIN TRY EXEC (@sql); END TRY BEGIN CATCH END CATCH; \\
           FETCH NEXT FROM c INTO @n; END; \\
         CLOSE c; DEALLOCATE c;",
    );
    Ok(server)
}

/// 管理用の接続で 1 つのバッチを実行する。呼び出し元が非同期テストでも動くよう、専用のスレッドで行う。
fn admin(server: &Server, sql: &str) -> Result<(), String> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<(), String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                rt.block_on(async {
                    let mut c = Config::new();
                    c.host(&server.host);
                    c.port(server.port);
                    c.authentication(AuthMethod::sql_server(
                        &server.user,
                        server.password.as_deref().unwrap_or(""),
                    ));
                    c.encryption(EncryptionLevel::NotSupported);
                    let tcp = TcpStream::connect((server.host.as_str(), server.port))
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut client = Client::connect(c, tcp.compat_write())
                        .await
                        .map_err(|e| e.to_string())?;
                    client
                        .simple_query(sql)
                        .await
                        .map_err(|e| e.to_string())?
                        .into_results()
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })
            })
            .join()
            .map_err(|_| "データベース作成のスレッドが panic しました".to_string())?
    })
}

/// `10^digits` 行の連番 `g`(0 始まり)を作る式。`FROM` の後ろにそのまま置ける(別名は `nums`)。
pub fn numbers(digits: u32) -> String {
    numbers_as(digits, "nums")
}

/// [`numbers`] の別名つき。同じ式を何度も結合するときに使う。
pub fn numbers_as(digits: u32, alias: &str) -> String {
    let d = "(VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9))";
    let sum: Vec<String> = (0..digits)
        .map(|i| format!("t{i}.d * {}", 10u64.pow(i)))
        .collect();
    let from: Vec<String> = (0..digits).map(|i| format!("{d} t{i}(d)")).collect();
    format!(
        "(SELECT {} AS g FROM {}) {alias}",
        sum.join(" + "),
        from.join(", ")
    )
}
