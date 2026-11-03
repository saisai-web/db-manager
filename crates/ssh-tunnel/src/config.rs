//! トンネルの設定。

use std::path::PathBuf;
use std::time::Duration;

use driver_api::Secret;

#[derive(Debug, Clone)]
pub enum SshAuth {
    Password(Secret),
    /// 秘密鍵のファイル(OpenSSH 形式など)。鍵にパスフレーズがあれば指定する
    KeyFile {
        path: PathBuf,
        passphrase: Option<Secret>,
    },
}

#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// 利用者が信頼した、サーバーのホスト鍵の指紋(`SHA256:...`)。未登録なら `None`
    pub trusted_fingerprint: Option<String>,
    /// SSH サーバーから見た、転送先(DB サーバー)
    pub target_host: String,
    pub target_port: u16,
    pub connect_timeout: Duration,
}
