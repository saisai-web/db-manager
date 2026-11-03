use std::fmt;

pub type Result<T, E = DriverError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 接続できない(ネットワーク、TLS、タイムアウト)
    Connect,
    /// 認証に失敗した
    Auth,
    /// DB が文の実行でエラーを返した
    Query,
    /// 利用者の操作でキャンセルされた
    Cancelled,
    /// 接続が切れた
    Disconnected,
    /// このドライバが対応していない
    Unsupported,
    Other,
}

/// 画面に出せる形にしたドライバのエラー。接続文字列やパスワードは含めない。
#[derive(Debug, Clone)]
pub struct DriverError {
    pub kind: ErrorKind,
    pub message: String,
    /// SQLSTATE など DB のエラーコード
    pub code: Option<String>,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// 実行した文の先頭から数えた文字位置(0 始まり、バイトではなく文字)
    pub position: Option<usize>,
}

impl DriverError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            detail: None,
            hint: None,
            position: None,
        }
    }

    pub fn unsupported(what: impl fmt::Display) -> Self {
        Self::new(ErrorKind::Unsupported, format!("未対応です: {what}"))
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(code) = &self.code {
            write!(f, " ({code})")?;
        }
        Ok(())
    }
}

impl std::error::Error for DriverError {}
