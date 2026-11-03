//! MySQL / MariaDB ドライバ(mysql_async)。
//!
//! 結果はテキストプロトコルで受け取り、値を数値や日時型へ変換せず、サーバーが返した文字列のまま扱う。
//! 実行中の文の停止は、別の接続から `KILL QUERY` を送る。

mod dialect;
mod error;
mod metadata;
mod session;
mod types;

use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Capabilities, ConnectionConfig, Dialect, Driver, DriverError, ErrorKind, MetadataProvider,
    Result, Session, TlsMode,
};
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts};

pub use dialect::MysqlDialect;

#[derive(Debug, Default)]
pub struct MysqlDriver;

const CAPABILITIES: Capabilities = Capabilities {
    // MySQL では、スキーマはデータベースのこと。一覧にはサーバー上のデータベースを出す
    schemas: true,
    cancel: true,
    read_only_session: true,
    // DDL は暗黙にコミットされ、取り消せない
    transactional_ddl: false,
    affected_rows: true,
};

static DIALECT: MysqlDialect = MysqlDialect;

#[async_trait]
impl Driver for MysqlDriver {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    fn dialect(&self) -> &dyn Dialect {
        &DIALECT
    }

    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(session::MysqlSession::connect(cfg).await?))
    }

    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>> {
        Ok(Box::new(metadata::MysqlMetadata::connect(cfg).await?))
    }
}

/// 接続の設定。ソケットは使わず、TCP で接続する(SSH トンネル経由でも同じ経路になる)。
pub(crate) fn options(cfg: &ConnectionConfig) -> Result<Opts> {
    let connect_to = cfg
        .hostaddr
        .map_or_else(|| cfg.host.clone(), |ip| ip.to_string());
    let mut b = OptsBuilder::default()
        .ip_or_hostname(connect_to)
        .tcp_port(cfg.port.unwrap_or(3306))
        .user(cfg.username.clone())
        .pass(cfg.password.as_ref().map(|p| p.expose().to_string()))
        .db_name(cfg.database.clone())
        .prefer_socket(false)
        .tcp_nodelay(true);
    b = b.ssl_opts(ssl_options(cfg));
    Ok(b.into())
}

fn ssl_options(cfg: &ConnectionConfig) -> Option<SslOpts> {
    match cfg.tls {
        TlsMode::Disable => None,
        TlsMode::Verify => {
            let mut ssl = SslOpts::default();
            if let Some(ca) = &cfg.ca_file {
                ssl = ssl.with_root_certs(vec![ca.clone().into()]);
            }
            // SSH トンネル経由では 127.0.0.1 に接続するので、証明書は元のホスト名で検証する
            if cfg.hostaddr.is_some() {
                ssl = ssl.with_danger_tls_hostname_override(Some(cfg.host.clone()));
            }
            Some(ssl)
        }
        // 暗号化はするが、相手を確かめない。中間者に気づけないため、画面で警告する
        TlsMode::SkipVerify => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
    }
}

/// mysql_async は、TLS の暗号実装を「プロセスの既定」から取る。ワークスペースの別の依存が
/// 別の実装(aws-lc)を有効にしていると、既定が決まらず、TLS の設定で panic する。
/// このドライバが使う ring を、(まだ誰も設定していなければ)既定にする。
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// 接続を開く。時間内に確立できなければ失敗する。
pub(crate) async fn open(cfg: &ConnectionConfig) -> Result<Conn> {
    install_crypto_provider();
    let opts = options(cfg)?;
    match tokio::time::timeout(cfg.connect_timeout, Conn::new(opts)).await {
        Ok(Ok(conn)) => Ok(conn),
        Ok(Err(e)) => Err(error::connect_error(&e)),
        Err(_) => Err(DriverError::new(
            ErrorKind::Connect,
            format!(
                "{} 秒以内に接続できませんでした",
                cfg.connect_timeout.as_secs()
            ),
        )),
    }
}

/// 打ち切り後に、サーバーが止まって接続が空くのを待つ上限
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
