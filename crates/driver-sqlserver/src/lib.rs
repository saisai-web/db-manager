//! SQL Server ドライバ(tiberius)。
//!
//! 結果は、SQL Server が表示する形の文字列(バイナリだけは生のバイト列)で受け取り、
//! 数値や日時を f64 や日時型へ変換しない。実行中の文の停止は、TDS の Attention で行う。

mod dialect;
mod error;
mod explain;
mod metadata;
mod readonly;
mod session;
mod values;

use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Capabilities, ConnectionConfig, Dialect, Driver, DriverError, ErrorKind, MetadataProvider,
    Result, Session, TlsMode,
};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub use dialect::SqlServerDialect;

#[derive(Debug, Default)]
pub struct SqlServerDriver;

const CAPABILITIES: Capabilities = Capabilities {
    schemas: true,
    cancel: true,
    // サーバー側の読み取り専用セッションは無い。文ごとにトランザクションで包んで取り消す(`readonly`)
    read_only_session: true,
    transactional_ddl: true,
    affected_rows: true,
};

static DIALECT: SqlServerDialect = SqlServerDialect;

#[async_trait]
impl Driver for SqlServerDriver {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    fn dialect(&self) -> &dyn Dialect {
        &DIALECT
    }

    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(session::SqlServerSession::connect(cfg).await?))
    }

    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>> {
        Ok(Box::new(metadata::SqlServerMetadata::connect(cfg).await?))
    }
}

pub(crate) type Conn = Client<Compat<TcpStream>>;

const DEFAULT_PORT: u16 = 1433;

fn config(cfg: &ConnectionConfig) -> Config {
    let mut c = Config::new();
    c.host(&cfg.host);
    c.port(cfg.port.unwrap_or(DEFAULT_PORT));
    if let Some(db) = &cfg.database {
        c.database(db);
    }
    c.authentication(AuthMethod::sql_server(
        cfg.username.as_deref().unwrap_or(""),
        cfg.password.as_ref().map_or("", |p| p.expose()),
    ));
    c.application_name(&cfg.application_name);
    match cfg.tls {
        TlsMode::Disable => c.encryption(EncryptionLevel::NotSupported),
        TlsMode::Verify => {
            c.encryption(EncryptionLevel::Required);
            if let Some(ca) = &cfg.ca_file {
                c.trust_cert_ca(ca.display().to_string());
            }
        }
        // 暗号化はするが、相手を確かめない。中間者に気づけないため、画面で警告する
        TlsMode::SkipVerify => {
            c.encryption(EncryptionLevel::Required);
            c.trust_cert();
        }
    }
    c
}

/// 接続を開く。時間内に確立できなければ失敗する。
/// SSH トンネル経由(`hostaddr`)では、そこへ TCP で接続し、証明書は元のホスト名で検証する。
pub(crate) async fn open(cfg: &ConnectionConfig) -> Result<Conn> {
    let connect = async {
        let target = cfg
            .hostaddr
            .map_or_else(|| cfg.host.clone(), |ip| ip.to_string());
        let tcp = TcpStream::connect((target.as_str(), cfg.port.unwrap_or(DEFAULT_PORT)))
            .await
            .map_err(|_| DriverError::new(ErrorKind::Connect, "サーバーに接続できませんでした"))?;
        tcp.set_nodelay(true).ok();
        Client::connect(config(cfg), tcp.compat_write())
            .await
            .map_err(|e| error::connect_error(&e))
    };
    match tokio::time::timeout(cfg.connect_timeout, connect).await {
        Ok(r) => r,
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
