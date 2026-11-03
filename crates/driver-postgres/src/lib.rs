//! PostgreSQL ドライバ(tokio-postgres)。
//!
//! 結果は simple_query_raw で文字列のまま受け取り、値を f64 や日時型へ変換しない。
//! 列の型は、実行前に Parse だけを行って取得する。

mod bound;
mod dialect;
mod error;
mod metadata;
mod session;
mod tls;
mod types;

use async_trait::async_trait;
use driver_api::{
    Capabilities, ConnectionConfig, Dialect, Driver, MetadataProvider, Result, Session,
};
use tokio_postgres::{NoTls, config::SslMode};

pub use dialect::PostgresDialect;

#[derive(Debug, Default)]
pub struct PostgresDriver;

const CAPABILITIES: Capabilities = Capabilities {
    schemas: true,
    cancel: true,
    read_only_session: true,
    transactional_ddl: true,
    affected_rows: true,
};

static DIALECT: PostgresDialect = PostgresDialect;

#[async_trait]
impl Driver for PostgresDriver {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    fn dialect(&self) -> &dyn Dialect {
        &DIALECT
    }

    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(session::PgSession::connect(cfg).await?))
    }

    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>> {
        Ok(Box::new(metadata::PgMetadata::connect(cfg).await?))
    }
}

/// 接続の確立。接続を駆動するタスクと、暗号化の方法(キャンセルで使う)も返す。
pub(crate) async fn open(
    cfg: &ConnectionConfig,
) -> Result<(
    tokio_postgres::Client,
    tokio::task::JoinHandle<()>,
    tls::Connector,
)> {
    let connector = tls::connector(cfg)?;

    let mut pg = tokio_postgres::Config::new();
    pg.host(&cfg.host);
    if let Some(ip) = cfg.hostaddr {
        pg.hostaddr(ip);
    }
    if let Some(port) = cfg.port {
        pg.port(port);
    }
    if let Some(db) = &cfg.database {
        pg.dbname(db);
    }
    if let Some(user) = &cfg.username {
        pg.user(user);
    }
    if let Some(pw) = &cfg.password {
        pg.password(pw.expose());
    }
    pg.application_name(&cfg.application_name);
    pg.connect_timeout(cfg.connect_timeout);
    // 値を DB が返した文字列のまま扱うため、表記を固定する。
    // bytea は 16 進、日時は ISO(タイムゾーンのオフセットつき)で受け取る。
    pg.options("-c bytea_output=hex -c DateStyle=ISO");

    let (client, task) = match &connector {
        tls::Connector::Plain => {
            let (client, connection) = pg.connect(NoTls).await.map_err(error::connect_error)?;
            (
                client,
                tokio::spawn(async move {
                    let _ = connection.await;
                }),
            )
        }
        tls::Connector::Tls(tls) => {
            // TLS を要求する。サーバーが TLS に対応しなければ、平文には落とさず失敗する
            pg.ssl_mode(SslMode::Require);
            let (client, connection) = pg
                .connect(tls.clone())
                .await
                .map_err(error::connect_error)?;
            (
                client,
                tokio::spawn(async move {
                    let _ = connection.await;
                }),
            )
        }
    };
    Ok((client, task, connector))
}
