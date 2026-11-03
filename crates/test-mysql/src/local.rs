//! 使い捨ての `mysqld` を一時ディレクトリで起動する。

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Server;

pub fn start() -> Result<Server, String> {
    let mysqld = find_mysqld().ok_or("mysqld が見つかりません(SQLTOOL_MYSQLD で指定できます)")?;
    let root = tempfile::Builder::new()
        .prefix("sqltool-mysql-")
        .tempdir()
        .map_err(|e| e.to_string())?
        .keep();
    let data = root.join("data");
    let port = free_port()?;

    // root はパスワードなし。127.0.0.1 だけで待ち受けるので、外からは使えない
    let init = Command::new(&mysqld)
        .arg("--no-defaults")
        .arg("--initialize-insecure")
        .arg(format!("--datadir={}", data.display()))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !init.status.success() {
        return Err(format!(
            "mysqld の初期化に失敗しました: {}",
            String::from_utf8_lossy(&init.stderr)
        ));
    }

    let child = Command::new(&mysqld)
        .arg("--no-defaults")
        .arg(format!("--datadir={}", data.display()))
        .arg(format!("--port={port}"))
        .arg("--bind-address=127.0.0.1")
        .arg(format!("--socket={}", root.join("mysql.sock").display()))
        .arg(format!(
            "--mysqlx-socket={}",
            root.join("mysqlx.sock").display()
        ))
        .arg("--mysqlx=OFF")
        .arg("--skip-log-bin")
        .arg("--innodb-flush-log-at-trx-commit=0")
        .arg("--max-connections=300")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    // テストプロセスが終了したら、サーバーを止めて消す。Drop は静的変数には呼ばれないため、
    // 別プロセスで親の生存を見張る。
    let _ = Command::new("sh")
        .arg("-c")
        .arg(
            "while kill -0 \"$1\" 2>/dev/null; do sleep 1; done; \
             kill \"$2\" 2>/dev/null; sleep 2; rm -rf \"$3\"",
        )
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(child.id().to_string())
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    wait_until_listening(port)?;
    Ok(Server {
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        password: None,
    })
}

fn wait_until_listening(port: u16) -> Result<(), String> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            // 待ち受けを始めた直後は、まだ接続を受け付けないことがある
            std::thread::sleep(Duration::from_millis(500));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("mysqld が起動しませんでした".into())
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

fn find_mysqld() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SQLTOOL_MYSQLD") {
        return Some(PathBuf::from(p));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("mysqld"))
                .collect()
        })
        .unwrap_or_default();
    candidates.extend(
        [
            "/opt/homebrew/opt/mysql/bin/mysqld",
            "/usr/local/opt/mysql/bin/mysqld",
            "/usr/sbin/mysqld",
            "/usr/libexec/mysqld",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
