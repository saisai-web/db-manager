//! トンネルを開けなかった理由。利用者に見せる文言は、呼び出し側(core)が付ける。

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TunnelError {
    /// 初めて接続するサーバー。指紋を利用者に確認してもらい、信頼するなら登録してやり直す。
    #[error("SSH サーバーのホスト鍵が未登録です({fingerprint})")]
    HostKeyUnknown { fingerprint: String },
    /// 登録済みの指紋と違う。なりすましの可能性があるので、続けない。
    #[error("SSH サーバーのホスト鍵が登録済みのものと違います(登録: {expected} / 今回: {actual})")]
    HostKeyChanged { expected: String, actual: String },
    #[error("SSH の認証に失敗しました(ユーザー名、パスワード、鍵を確認してください)")]
    AuthFailed,
    #[error("秘密鍵を読めません: {0}")]
    KeyFile(String),
    #[error("SSH サーバーに接続できません: {0}")]
    Connect(String),
    #[error("SSH サーバーへの接続がタイムアウトしました")]
    Timeout,
}
