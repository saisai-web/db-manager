//! 使い捨ての sshd(127.0.0.1、鍵認証のみ)を起動して、実際にトンネルを通す。
//! `/usr/sbin/sshd` と `ssh-keygen` が無い環境では、何も確かめずに終える。

use std::time::Duration;

use driver_api::Secret;
use ssh_tunnel::{SshAuth, Tunnel, TunnelConfig, TunnelError};
use test_ssh::{Sshd, free_port, keygen};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// テスト用 sshd に、このクレートの設定を組み立てる。
fn config(sshd: &Sshd, key: &str, passphrase: Option<&str>, target_port: u16) -> TunnelConfig {
    TunnelConfig {
        ssh_host: "127.0.0.1".into(),
        ssh_port: sshd.port,
        user: Sshd::user_name(),
        auth: SshAuth::KeyFile {
            path: sshd.path(key),
            passphrase: passphrase.map(Secret::new),
        },
        trusted_fingerprint: Some(sshd.host_fingerprint()),
        target_host: "127.0.0.1".into(),
        target_port,
        connect_timeout: Duration::from_secs(10),
    }
}

/// 受け取った内容に "echo:" を付けて返すサーバー(転送先の代わり)。
async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 {
                        return;
                    }
                    let mut out = b"echo:".to_vec();
                    out.extend_from_slice(&buf[..n]);
                    if s.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

async fn ask(port: u16, text: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(text.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 5 + text.len()];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut buf))
        .await
        .expect("reply in time")
        .unwrap();
    String::from_utf8(buf).unwrap()
}

#[tokio::test]
async fn data_flows_through_the_tunnel_to_the_target() {
    let Some(sshd) = Sshd::start() else { return };
    let target = echo_server().await;
    let tunnel = Tunnel::open(&config(&sshd, "user", None, target))
        .await
        .unwrap();
    assert_ne!(tunnel.local_port(), target);
    assert_eq!(ask(tunnel.local_port(), "hello").await, "echo:hello");
    // 同時に複数の接続も通せる
    let (a, b) = tokio::join!(
        ask(tunnel.local_port(), "one"),
        ask(tunnel.local_port(), "two")
    );
    assert_eq!((a.as_str(), b.as_str()), ("echo:one", "echo:two"));
}

#[tokio::test]
async fn a_passphrase_protected_key_works_and_a_wrong_passphrase_is_reported() {
    let Some(sshd) = Sshd::start() else { return };
    let target = echo_server().await;
    let tunnel = Tunnel::open(&config(&sshd, "locked", Some("s3cret"), target))
        .await
        .unwrap();
    assert_eq!(ask(tunnel.local_port(), "x").await, "echo:x");
    let err = Tunnel::open(&config(&sshd, "locked", Some("wrong"), target))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, TunnelError::KeyFile(_)), "{err:?}");
    let err = Tunnel::open(&config(&sshd, "locked", None, target))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, TunnelError::KeyFile(_)), "{err:?}");
}

#[tokio::test]
async fn an_unknown_host_key_stops_before_authentication_and_reports_the_fingerprint() {
    let Some(sshd) = Sshd::start() else { return };
    let mut cfg = config(&sshd, "user", None, 1);
    cfg.trusted_fingerprint = None;
    let err = Tunnel::open(&cfg).await.err().unwrap();
    assert_eq!(
        err,
        TunnelError::HostKeyUnknown {
            fingerprint: sshd.host_fingerprint()
        }
    );
}

#[tokio::test]
async fn a_changed_host_key_is_refused() {
    let Some(sshd) = Sshd::start() else { return };
    let mut cfg = config(&sshd, "user", None, 1);
    cfg.trusted_fingerprint = Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
    let err = Tunnel::open(&cfg).await.err().unwrap();
    assert_eq!(
        err,
        TunnelError::HostKeyChanged {
            expected: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            actual: sshd.host_fingerprint()
        }
    );
}

#[tokio::test]
async fn an_unauthorized_key_fails_authentication() {
    let Some(sshd) = Sshd::start() else { return };
    keygen(&sshd.path("stranger"), "");
    let err = Tunnel::open(&config(&sshd, "stranger", None, 1))
        .await
        .err()
        .unwrap();
    assert_eq!(err, TunnelError::AuthFailed);
}

#[tokio::test]
async fn an_unreachable_server_is_a_connect_error_and_a_missing_key_file_is_reported() {
    let Some(sshd) = Sshd::start() else { return };
    let mut cfg = config(&sshd, "user", None, 1);
    cfg.ssh_port = free_port();
    let err = Tunnel::open(&cfg).await.err().unwrap();
    assert!(matches!(err, TunnelError::Connect(_)), "{err:?}");
    let err = Tunnel::open(&config(&sshd, "no-such-key", None, 1))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, TunnelError::KeyFile(_)), "{err:?}");
}

#[tokio::test]
async fn dropping_the_tunnel_closes_the_local_port() {
    let Some(sshd) = Sshd::start() else { return };
    let target = echo_server().await;
    let tunnel = Tunnel::open(&config(&sshd, "user", None, target))
        .await
        .unwrap();
    let port = tunnel.local_port();
    drop(tunnel);
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the local port is still open after the tunnel was dropped");
}
