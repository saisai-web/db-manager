//! Oracle Database ドライバ(`oracledb`: Oracle 製の、純 Rust の thin ドライバ。Oracle Client は要らない)。
//!
//! `oracledb` は同期 API なので、接続ごとに専用のスレッドで動かし、結果をチャネルで流す。
//! 結果の値は、数値を f64 にせず、Oracle が返した内容のまま文字列にする。
//!
//! 注意(`oracledb` はベータ版):
//! * 実行中の文を止める API がない。キャンセルは、別の接続から `ALTER SYSTEM KILL SESSION` で
//!   セッションごと終了させる(`ALTER SYSTEM` 権限が要り、トランザクションは失われる)。
//! * `TIMESTAMP WITH TIME ZONE` は、ライブラリが UTC に直して返す(元のオフセットは分からない)。

mod dialect;
mod error;
mod explain;
mod metadata;
mod session;
mod tls;
mod txn;
mod values;
mod worker;

use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Capabilities, ConnectionConfig, Dialect, Driver, DriverError, ErrorKind, MetadataProvider,
    Result, Session, TlsMode,
};

pub use dialect::OracleDialect;

#[derive(Debug, Default)]
pub struct OracleDriver;

const CAPABILITIES: Capabilities = Capabilities {
    // Oracle では、スキーマは利用者のこと
    schemas: true,
    // 別の接続からセッションを終了させる方法のみ。権限がないと、キャンセルできない
    cancel: true,
    // セッション全体の設定はないので、文ごとの `SET TRANSACTION READ ONLY` + ROLLBACK で実現する
    // (`txn` モジュール)
    read_only_session: true,
    // DDL は暗黙にコミットされ、取り消せない
    transactional_ddl: false,
    affected_rows: true,
};

static DIALECT: OracleDialect = OracleDialect;

#[async_trait]
impl Driver for OracleDriver {
    fn capabilities(&self) -> &Capabilities {
        &CAPABILITIES
    }

    fn dialect(&self) -> &dyn Dialect {
        &DIALECT
    }

    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>> {
        Ok(Box::new(session::OracleSession::connect(cfg).await?))
    }

    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>> {
        Ok(Box::new(metadata::OracleMetadata::connect(cfg).await?))
    }
}

/// 接続の設定。`database` はサービス名。TLS は `tcps`(組み込みの CA と `ca_file` で、サーバーの証明書を検証する)。
pub(crate) fn config(cfg: &ConnectionConfig) -> Result<oracledb::Config> {
    let service = cfg
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DriverError::new(ErrorKind::Connect, "サービス名を指定してください"))?;
    let scheme = match cfg.tls {
        TlsMode::Disable => "tcp",
        TlsMode::SkipVerify => {
            return Err(DriverError::unsupported(
                "Oracle では、証明書を検証しない TLS は使えません(「暗号化しない」か「証明書を検証」を選んでください)",
            ));
        }
        TlsMode::Verify => {
            if cfg.hostaddr.is_some() {
                return Err(DriverError::unsupported(
                    "Oracle では、SSH トンネル経由の TLS 検証はできません(サーバー名が 127.0.0.1 になるため)",
                ));
            }
            install_crypto_provider();
            "tcps"
        }
    };
    let host = cfg
        .hostaddr
        .map_or_else(|| cfg.host.clone(), |ip| ip.to_string());
    let port = cfg.port.unwrap_or(1521);
    let mut config = oracledb::Config::default()
        .set_credentials(
            cfg.username.as_deref().unwrap_or(""),
            cfg.password.as_ref().map_or("", |p| p.expose()),
        )
        .set_connect_string(&format!("{scheme}://{host}:{port}/{service}"))
        .map_err(|e| error::connect_error(&e))?
        .set_transport_connect_timeout(Some(cfg.connect_timeout));
    if let (TlsMode::Verify, Some(ca_file)) = (cfg.tls, &cfg.ca_file) {
        config = config.set_wallet_location(tls::wallet_dir(ca_file)?);
    }
    if let Ok(c) = config.clone().set_program(cfg.application_name.clone()) {
        config = c;
    }
    Ok(config)
}

/// `oracledb` は、TLS のとき、rustls のプロセス既定の暗号プロバイダを使う。アプリ全体では
/// ring と aws-lc-rs の両方が有効になり、自動では選べず、パニックするので、ring を既定にする。
/// (ほかのドライバは、プロバイダを明示して使うので、影響を受けない)
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// 接続を開く(ブロックする)。呼び出し元は `spawn_blocking` などで呼ぶ。
pub(crate) fn open_blocking(cfg: &ConnectionConfig) -> Result<oracledb::Connection> {
    let conn = oracledb::connect(config(cfg)?).map_err(|e| error::connect_error(&e))?;
    // 値の書式を固定する。バインドする文字列を、列の型に直すときに使われる(`OracleDialect::bind_text`)
    conn.execute(
        "ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,' \
         NLS_DATE_FORMAT = 'YYYY-MM-DD HH24:MI:SS' \
         NLS_TIMESTAMP_FORMAT = 'YYYY-MM-DD HH24:MI:SS.FF' \
         NLS_TIMESTAMP_TZ_FORMAT = 'YYYY-MM-DD HH24:MI:SS.FF TZR'",
        &[],
    )
    .map_err(|e| error::connect_error(&e))?;
    Ok(conn)
}

/// 停止の要求を送る、別の接続を開く上限
pub(crate) const KILL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
