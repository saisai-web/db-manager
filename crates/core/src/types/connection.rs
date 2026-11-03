//! 接続設定の型。

use super::{ConnectionId, ConnectionOptions, EnvLabel, Secret};

/// 画面に出す接続設定。パスワードも参照キーも含まない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSummary {
    pub id: ConnectionId,
    pub name: String,
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub options: ConnectionOptions,
    pub env_label: EnvLabel,
    pub has_password: bool,
}

#[derive(Debug, Clone, Default)]
pub enum PasswordChange {
    /// 保存済みのパスワードを変えない(新規なら、パスワードなし)
    #[default]
    Keep,
    Set(Secret),
    Clear,
}

#[derive(Debug, Clone)]
pub struct ConnectionInput {
    /// 既存の接続を更新するときの id
    pub id: Option<ConnectionId>,
    pub name: String,
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub options: ConnectionOptions,
    pub env_label: EnvLabel,
    pub password: PasswordChange,
    /// SSH トンネルのパスワード(鍵ファイルを使うときは、鍵のパスフレーズ)
    pub ssh_secret: PasswordChange,
}
