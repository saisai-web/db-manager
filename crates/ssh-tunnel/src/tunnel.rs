//! トンネル本体。SSH で接続し、ローカルで受けた接続を、`direct-tcpip` で転送先へつなぐ。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::config::{SshAuth, TunnelConfig};
use crate::error::TunnelError;

/// サーバーが示したホスト鍵の指紋を調べる。登録済みと一致したときだけ、先へ進む。
struct HostKeyCheck {
    trusted: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

fn fingerprint_of(key: &PublicKeyOrCertificate) -> String {
    let public = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        PublicKeyOrCertificate::Certificate(c) => c.public_key().clone().into(),
    };
    public.fingerprint(HashAlg::Sha256).to_string()
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let actual = fingerprint_of(key);
        let ok = self.trusted.as_deref() == Some(actual.as_str());
        *self.seen.lock().expect("fingerprint lock") = Some(actual);
        Ok(ok)
    }
}

/// 開いているトンネル。捨てると、待ち受けと SSH 接続を閉じる。
pub struct Tunnel {
    local_port: u16,
    session: Arc<Handle<HostKeyCheck>>,
    accept: JoinHandle<()>,
}

impl Tunnel {
    /// SSH サーバーに接続して認証し、`127.0.0.1` の空きポートで待ち受ける。
    pub async fn open(cfg: &TunnelConfig) -> Result<Self, TunnelError> {
        let timeout = cfg.connect_timeout;
        let session = tokio::time::timeout(timeout, connect(cfg))
            .await
            .map_err(|_| TunnelError::Timeout)??;

        // 待ち受けは、ループバックに限る
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| TunnelError::Connect(e.to_string()))?;
        let local_port = listener
            .local_addr()
            .map_err(|e| TunnelError::Connect(e.to_string()))?
            .port();
        let target = (cfg.target_host.clone(), cfg.target_port);
        let session = Arc::new(session);
        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&session), target));
        Ok(Self {
            local_port,
            session,
            accept,
        })
    }

    /// SSH の接続が切れている(再接続が要る)か。
    pub fn is_closed(&self) -> bool {
        self.session.is_closed() || self.accept.is_finished()
    }

    /// DB ドライバには、ここ(`127.0.0.1:local_port`)へ接続させる。
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // 待ち受けを止めると、SSH の接続(ここが持つ唯一の参照)も閉じる
        self.accept.abort();
    }
}

async fn connect(cfg: &TunnelConfig) -> Result<Handle<HostKeyCheck>, TunnelError> {
    let seen = Arc::new(Mutex::new(None));
    let handler = HostKeyCheck {
        trusted: cfg.trusted_fingerprint.clone(),
        seen: seen.clone(),
    };
    let config = Arc::new(client::Config {
        nodelay: true,
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let connected = client::connect(config, (cfg.ssh_host.as_str(), cfg.ssh_port), handler).await;
    let mut session = match connected {
        Ok(s) => s,
        Err(e) => return Err(explain(e, &cfg.trusted_fingerprint, &seen)),
    };

    let authenticated = match &cfg.auth {
        SshAuth::Password(pw) => {
            session
                .authenticate_password(cfg.user.clone(), pw.expose())
                .await
        }
        SshAuth::KeyFile { path, passphrase } => {
            let key = load_secret_key(path, passphrase.as_ref().map(|p| p.expose()))
                .map_err(|e| TunnelError::KeyFile(e.to_string()))?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|e| TunnelError::Connect(e.to_string()))?
                .flatten();
            session
                .authenticate_publickey(
                    cfg.user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
        }
    };
    match authenticated {
        Ok(r) if r.success() => Ok(session),
        Ok(_) => Err(TunnelError::AuthFailed),
        Err(e) => Err(TunnelError::Connect(e.to_string())),
    }
}

/// 接続の失敗が、ホスト鍵の確認で止めたものなら、その理由を返す。
fn explain(
    err: russh::Error,
    trusted: &Option<String>,
    seen: &Mutex<Option<String>>,
) -> TunnelError {
    let actual = seen.lock().expect("fingerprint lock").clone();
    match (err, actual, trusted) {
        (russh::Error::UnknownKey, Some(actual), None) => TunnelError::HostKeyUnknown {
            fingerprint: actual,
        },
        (russh::Error::UnknownKey, Some(actual), Some(expected)) => TunnelError::HostKeyChanged {
            expected: expected.clone(),
            actual,
        },
        (e, ..) => TunnelError::Connect(e.to_string()),
    }
}

async fn accept_loop(
    listener: TcpListener,
    session: Arc<Handle<HostKeyCheck>>,
    target: (String, u16),
) {
    loop {
        let Ok((socket, peer)) = listener.accept().await else {
            return;
        };
        let (session, target) = (session.clone(), target.clone());
        tokio::spawn(async move {
            let _ = forward(socket, peer.port(), &session, &target).await;
        });
    }
}

async fn forward(
    mut socket: TcpStream,
    peer_port: u16,
    session: &Handle<HostKeyCheck>,
    (host, port): &(String, u16),
) -> Result<(), russh::Error> {
    let channel = session
        .channel_open_direct_tcpip(
            host.clone(),
            u32::from(*port),
            "127.0.0.1",
            u32::from(peer_port),
        )
        .await?;
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
    Ok(())
}
