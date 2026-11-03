//! SQLite ドライバ(rusqlite)。同期 API のため、専用スレッドで動かす。
//!
//! 接続先は、ファイルのパス(`ConnectionConfig::host`)。空なら、メモリ上の DB。
//! ホスト、ユーザー、パスワード、TLS の設定は使わない。

mod dialect;
mod error;
mod metadata;
mod session;
mod values;
mod worker;

use async_trait::async_trait;
use driver_api::{
    Capabilities, ConnectionConfig, Dialect, Driver, MetadataProvider, Result, Session,
};

pub use dialect::SqliteDialect;

#[derive(Debug, Default)]
pub struct SqliteDriver;

const CAPABILITIES: Capabilities = Capabilities {
    schemas: true,
    cancel: true,
    read_only_session: true,
    transactional_ddl: true,
    affected_rows: true,
};

static DIALECT: SqliteDialect = SqliteDialect;

fn path(cfg: &ConnectionConfig) -> &str {
    match cfg.host.trim() {
        "" => ":memory:",
        p => p,
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    fn dialect(&self) -> &dyn Dialect {
        &DIALECT
    }

    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>> {
        let p = path(cfg).to_string();
        // ファイルを開く処理は同期なので、専用のスレッドに任せる
        let session = tokio::task::spawn_blocking(move || session::SqliteSession::open(&p))
            .await
            .map_err(|e| {
                driver_api::DriverError::new(driver_api::ErrorKind::Other, e.to_string())
            })??;
        Ok(Box::new(session))
    }

    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>> {
        let p = path(cfg).to_string();
        let meta = tokio::task::spawn_blocking(move || metadata::SqliteMetadata::open(&p))
            .await
            .map_err(|e| {
                driver_api::DriverError::new(driver_api::ErrorKind::Other, e.to_string())
            })??;
        Ok(Box::new(meta))
    }
}
