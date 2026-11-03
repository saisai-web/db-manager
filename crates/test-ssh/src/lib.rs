//! 使い捨ての sshd を、127.0.0.1 の空きポートで起動する(鍵認証のみ、現在のユーザーとして)。
//! `/usr/sbin/sshd` と `ssh-keygen` が無い環境では [`Sshd::start`] が `None` を返す。
//! 呼び出し側のテストは、そのとき何も確かめずに終える(`test-pg` と同じ流儀)。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SSHD: &str = "/usr/sbin/sshd";

/// パスフレーズつきの鍵のパスフレーズ。
pub const LOCKED_KEY_PASSPHRASE: &str = "s3cret";

pub struct Sshd {
    dir: tempfile::TempDir,
    pub port: u16,
    child: Child,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn keygen(path: &Path, passphrase: &str) {
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "ssh-keygen failed");
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// 公開鍵ファイルの指紋(`SHA256:...`)。
pub fn fingerprint_of(pub_file: &Path) -> String {
    let out = Command::new("ssh-keygen")
        .arg("-lf")
        .arg(pub_file)
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string()
}

impl Sshd {
    pub fn start() -> Option<Self> {
        if !Path::new(SSHD).exists() || Command::new("ssh-keygen").arg("-?").output().is_err() {
            eprintln!("skipped: no sshd / ssh-keygen");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        keygen(&p.join("host"), "");
        keygen(&p.join("user"), "");
        keygen(&p.join("locked"), LOCKED_KEY_PASSPHRASE);
        std::fs::write(
            p.join("authorized_keys"),
            format!(
                "{}{}",
                std::fs::read_to_string(p.join("user.pub")).unwrap(),
                std::fs::read_to_string(p.join("locked.pub")).unwrap()
            ),
        )
        .unwrap();
        let port = free_port();
        std::fs::write(
            p.join("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nPidFile {pid}\n\
                 AuthorizedKeysFile {keys}\nPasswordAuthentication no\n\
                 KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\n\
                 StrictModes no\nAllowTcpForwarding yes\nLogLevel ERROR\n",
                host = p.join("host").display(),
                pid = p.join("sshd.pid").display(),
                keys = p.join("authorized_keys").display(),
            ),
        )
        .unwrap();
        let child = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(p.join("sshd_config"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let me = Self { dir, port, child };
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(me);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("sshd did not start");
    }

    /// 作業ディレクトリの中のファイル(`user`、`locked`、`host.pub` など)。
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn host_fingerprint(&self) -> String {
        fingerprint_of(&self.path("host.pub"))
    }

    /// ログインするユーザー名(sshd を起動したユーザー)。
    pub fn user_name() -> String {
        std::env::var("USER").unwrap_or_else(|_| "root".into())
    }
}
