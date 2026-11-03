//! 結合テスト用の PostgreSQL サーバー。
//!
//! 接続先は次の順で決まる。
//!
//! 1. 環境変数 `SQLTOOL_TEST_PG_HOST`(と `_PORT`、`_USER`、`_PASSWORD`)があれば、
//!    そのサーバーを使う。CI のサービスコンテナ向け。
//! 2. なければ、PATH や Homebrew などから見つけた PostgreSQL で、一時ディレクトリに
//!    使い捨てのクラスタを作って起動する(Unix のみ)。テストプロセスが終了すると、
//!    見張りのプロセスがクラスタを停止して削除する。
//!
//! どちらの場合も、テストごとに専用のデータベースを作って返す。

#[cfg(unix)]
use std::net::TcpListener;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone)]
pub struct TestDb {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub dbname: String,
    /// TLS で接続するときの、サーバー証明書(自己署名。CA としても使える)。TLS を使えなければ `None`
    pub ssl_cert: Option<PathBuf>,
}

struct Server {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    ssl_cert: Option<PathBuf>,
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

    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&server.host)
        .port(server.port)
        .user(&server.user)
        .dbname("postgres");
    if let Some(pw) = &server.password {
        cfg.password(pw);
    }
    // 呼び出し元が非同期テストでも動くよう、専用のスレッドで同期的に実行する
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> Result<(), String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                rt.block_on(async {
                    let (client, conn) = cfg
                        .connect(tokio_postgres::NoTls)
                        .await
                        .map_err(|e| e.to_string())?;
                    let task = tokio::spawn(conn);
                    client
                        .batch_execute(&format!("CREATE DATABASE {dbname}"))
                        .await
                        .map_err(|e| e.to_string())?;
                    drop(client);
                    let _ = task.await;
                    Ok(())
                })
            })
            .join()
            .map_err(|_| "データベース作成のスレッドが panic しました".to_string())?
    })?;

    Ok(TestDb {
        host: server.host.clone(),
        port: server.port,
        user: server.user.clone(),
        password: server.password.clone(),
        dbname,
        ssl_cert: server.ssl_cert.clone(),
    })
}

/// DB を用意できないときにテストを飛ばすための入口。環境変数
/// `SQLTOOL_REQUIRE_PG=1` なら、飛ばさずに失敗させる(CI で見落とさないため)。
pub fn database_or_skip() -> Option<TestDb> {
    match create_database() {
        Ok(db) => Some(db),
        Err(e) if std::env::var_os("SQLTOOL_REQUIRE_PG").is_some() => {
            panic!("PostgreSQL が必要です: {e}")
        }
        Err(e) => {
            eprintln!("SKIP: PostgreSQL を用意できません: {e}");
            None
        }
    }
}

fn start_server() -> Result<Server, String> {
    if let Ok(host) = std::env::var("SQLTOOL_TEST_PG_HOST") {
        return Ok(Server {
            host,
            port: std::env::var("SQLTOOL_TEST_PG_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(5432),
            user: std::env::var("SQLTOOL_TEST_PG_USER").unwrap_or_else(|_| "postgres".into()),
            password: std::env::var("SQLTOOL_TEST_PG_PASSWORD").ok(),
            // TLS の試験は、サーバー証明書(PEM)の場所を指定したときだけ行う
            ssl_cert: std::env::var_os("SQLTOOL_TEST_PG_SSL_CERT").map(PathBuf::from),
        });
    }
    start_local_cluster()
}

#[cfg(not(unix))]
fn start_local_cluster() -> Result<Server, String> {
    Err("Windows では SQLTOOL_TEST_PG_HOST で接続先を指定してください".into())
}

#[cfg(unix)]
fn start_local_cluster() -> Result<Server, String> {
    use std::process::Stdio;
    let bin = find_bin_dir().ok_or("initdb が見つかりません(SQLTOOL_PG_BIN_DIR で指定できます)")?;
    let root = tempfile::Builder::new()
        .prefix("sqltool-pg-")
        .tempdir()
        .map_err(|e| e.to_string())?
        .keep();
    let data = root.join("data");
    let port = free_port()?;

    run(Command::new(bin.join("initdb"))
        .args(["-D"])
        .arg(&data)
        .args(["-U", "postgres", "--auth=trust", "-E", "UTF8", "--locale=C"]))?;

    // TLS 用の自己署名証明書。openssl がなければ、TLS なしで起動する
    let ssl_cert = make_certificate(&root);
    let ssl_options = match &ssl_cert {
        Some(_) => format!(
            " -c ssl=on -c ssl_cert_file={} -c ssl_key_file={}",
            root.join("server.crt").display(),
            root.join("server.key").display()
        ),
        None => String::new(),
    };
    // ソケットは使わず、127.0.0.1 だけで待ち受ける。テスト用なので fsync は不要
    let options = format!(
        "-p {port} -c listen_addresses=127.0.0.1 -c unix_socket_directories={} \
         -c fsync=off -c max_connections=200{ssl_options}",
        root.display()
    );
    run(Command::new(bin.join("pg_ctl"))
        .args(["-D"])
        .arg(&data)
        .args(["-w", "-l"])
        .arg(root.join("log"))
        .args(["-o", &options, "start"]))?;

    // テストプロセスが終了したら、クラスタを停止して消す。Drop は静的変数には呼ばれないため、
    // 別プロセスで親の生存を見張る。
    let _ = Command::new("sh")
        .arg("-c")
        .arg(
            "while kill -0 \"$1\" 2>/dev/null; do sleep 1; done; \
             \"$2\" -D \"$3\" -m immediate stop >/dev/null 2>&1; rm -rf \"$4\"",
        )
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(bin.join("pg_ctl"))
        .arg(&data)
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    Ok(Server {
        host: "127.0.0.1".into(),
        port,
        user: "postgres".into(),
        password: None,
        ssl_cert,
    })
}

/// テスト用の CA と、それが署名したサーバー証明書(localhost / 127.0.0.1 向け)を作る。
/// サーバーが使うのは `server.crt`、クライアントが信頼するのは CA(戻り値)。
/// 自己署名の証明書をサーバー証明書にすると、CA 属性があるため、検証側が拒否する。
/// openssl がなければ `None`(TLS なしで起動する)。
#[cfg(unix)]
fn make_certificate(root: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let p = |name: &str| root.join(name);
    let openssl = |args: &[&std::ffi::OsStr]| {
        Command::new("openssl")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let s = |x: &'static str| std::ffi::OsStr::new(x);
    std::fs::write(
        p("ext.cnf"),
        "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\n",
    )
    .ok()?;
    let (ca_crt, ca_key, srv_key, srv_csr, srv_crt, ext) = (
        p("ca.crt"),
        p("ca.key"),
        p("server.key"),
        p("server.csr"),
        p("server.crt"),
        p("ext.cnf"),
    );
    openssl(&[
        s("req"),
        s("-new"),
        s("-x509"),
        s("-days"),
        s("2"),
        s("-nodes"),
        s("-subj"),
        s("/CN=sqltool-test-ca"),
        s("-keyout"),
        ca_key.as_os_str(),
        s("-out"),
        ca_crt.as_os_str(),
    ])?;
    openssl(&[
        s("req"),
        s("-new"),
        s("-nodes"),
        s("-subj"),
        s("/CN=localhost"),
        s("-keyout"),
        srv_key.as_os_str(),
        s("-out"),
        srv_csr.as_os_str(),
    ])?;
    openssl(&[
        s("x509"),
        s("-req"),
        s("-in"),
        srv_csr.as_os_str(),
        s("-CA"),
        ca_crt.as_os_str(),
        s("-CAkey"),
        ca_key.as_os_str(),
        s("-CAcreateserial"),
        s("-days"),
        s("2"),
        s("-extfile"),
        ext.as_os_str(),
        s("-out"),
        srv_crt.as_os_str(),
    ])?;
    // PostgreSQL は、他のユーザーが読める秘密鍵を拒否する
    std::fs::set_permissions(&srv_key, std::fs::Permissions::from_mode(0o600)).ok()?;
    Some(ca_crt)
}

#[cfg(unix)]
fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?} が失敗しました: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

#[cfg(unix)]
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

#[cfg(unix)]
fn find_bin_dir() -> Option<PathBuf> {
    let has_initdb = |d: &Path| d.join("initdb").is_file();
    if let Some(d) = std::env::var_os("SQLTOOL_PG_BIN_DIR").map(PathBuf::from) {
        return has_initdb(&d).then_some(d);
    }
    if let Some(path) = std::env::var_os("PATH")
        && let Some(d) = std::env::split_paths(&path).find(|d| has_initdb(d))
    {
        return Some(d);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for base in ["/opt/homebrew/opt", "/usr/local/opt"] {
        if let Ok(rd) = std::fs::read_dir(base) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("postgresql") {
                    candidates.push(e.path().join("bin"));
                }
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir("/usr/lib/postgresql") {
        for e in rd.flatten() {
            candidates.push(e.path().join("bin"));
        }
    }
    candidates.sort();
    candidates.into_iter().rev().find(|d| has_initdb(d))
}
