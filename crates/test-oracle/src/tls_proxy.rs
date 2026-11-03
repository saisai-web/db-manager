//! TLS で待ち受けて、平文の Oracle へ中継する。
//!
//! テスト用の Oracle は TLS(tcps)を待ち受けていない。TLS の接続を、実際の Oracle で確かめるため、
//! テスト用の CA で署名した証明書を持つ中継を立てる。証明書は `openssl` コマンドで作る(無ければ `None`)。

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub struct TlsProxy {
    pub port: u16,
    /// 中継の証明書に署名した CA(PEM)。クライアントの `ca_file` に渡す
    pub ca_file: PathBuf,
    dir: PathBuf,
    task: JoinHandle<()>,
}

impl Drop for TlsProxy {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn openssl(dir: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("openssl")
        .args(args)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

impl TlsProxy {
    /// `localhost` の証明書で待ち受け、`target` の平文のサーバーへ中継する。
    /// `openssl` が使えなければ `None`。
    pub async fn start(target_host: &str, target_port: u16) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!(
            "sqltool-tls-proxy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(dir.join("ext.cnf"), "subjectAltName=DNS:localhost\n").ok()?;
        let made = openssl(
            &dir,
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "ca.key",
                "-out",
                "ca.pem",
                "-subj",
                "/CN=sqltool-test-ca",
                "-days",
                "2",
            ],
        ) && openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-subj",
                "/CN=localhost",
            ],
        ) && openssl(
            &dir,
            &[
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                "server.pem",
                "-days",
                "2",
                "-extfile",
                "ext.cnf",
            ],
        );
        if !made {
            let _ = std::fs::remove_dir_all(&dir);
            return None;
        }
        let certs: Vec<CertificateDer<'static>> =
            CertificateDer::pem_file_iter(dir.join("server.pem"))
                .ok()?
                .collect::<Result<_, _>>()
                .ok()?;
        let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).ok()?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .ok()?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.ok()?;
        let port = listener.local_addr().ok()?.port();
        let target = format!("{target_host}:{target_port}");
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (acceptor, target) = (acceptor.clone(), target.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(client).await else {
                        return;
                    };
                    let Ok(mut upstream) = TcpStream::connect(&target).await else {
                        return;
                    };
                    let _ = copy_bidirectional(&mut tls, &mut upstream).await;
                });
            }
        });
        Some(Self {
            port,
            ca_file: dir.join("ca.pem"),
            dir,
            task,
        })
    }
}
