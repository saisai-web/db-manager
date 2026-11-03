use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::StorageError;

/// 接続先の環境。本番ラベルは画面の赤枠と読み取り専用の既定を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvLabel {
    Dev,
    Staging,
    Production,
}

impl EnvLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StorageError> {
        match s {
            "dev" => Ok(Self::Dev),
            "staging" => Ok(Self::Staging),
            "production" => Ok(Self::Production),
            other => Err(StorageError::InvalidData(format!("env_label: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsSetting {
    Disable,
    /// 証明書とホスト名を検証する(既定)
    #[default]
    Verify,
    SkipVerify,
}

/// `connections.options`(JSON)。知らないキーは、新しい版で書かれたものとして保つ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionOptions {
    #[serde(default)]
    pub tls: TlsSetting,
    #[serde(default = "default_timeout")]
    pub connect_timeout_secs: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_timeout() -> u32 {
    10
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            tls: TlsSetting::default(),
            connect_timeout_secs: default_timeout(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnection {
    pub name: String,
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    pub options: ConnectionOptions,
    pub env_label: EnvLabel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    /// 資格情報ストアでの参照キー。パスワード自体ではない
    pub secret_ref: Option<String>,
    pub options: ConnectionOptions,
    pub env_label: EnvLabel,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStatus {
    Ok,
    Error,
    Cancelled,
}

impl HistoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StorageError> {
        match s {
            "ok" => Ok(Self::Ok),
            "error" => Ok(Self::Error),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StorageError::InvalidData(format!("status: {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHistory {
    pub connection_id: String,
    pub sql_text: String,
    pub duration_ms: Option<i64>,
    pub affected_rows: Option<i64>,
    pub status: HistoryStatus,
    /// 省略すると現在時刻(UTC、ミリ秒)
    pub executed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub connection_id: String,
    pub sql_text: String,
    pub executed_at: String,
    pub duration_ms: Option<i64>,
    pub affected_rows: Option<i64>,
    pub status: HistoryStatus,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryQuery {
    pub connection_id: Option<String>,
    /// SQL 文に含まれる文字列。英字の大文字小文字は区別しない(SQLite の LIKE の仕様で、ASCII のみ)
    pub contains: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub sql_text: String,
    pub updated_at: String,
}
