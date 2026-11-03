use std::fmt;
use std::time::Duration;

/// パスワードなどの秘密。`Debug` と `Display` では中身を出さない。
/// ログや画面に接続設定を出しても漏れないようにするための型。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 接続時にだけ使う。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// 暗号化しない
    Disable,
    /// 暗号化し、証明書とホスト名を検証する(既定)
    #[default]
    Verify,
    /// 暗号化するが証明書を検証しない。画面で警告すること
    SkipVerify,
}

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<Secret>,
    pub tls: TlsMode,
    /// 検証に使う CA 証明書(PEM)。社内 CA など、OS が信頼しない CA のサーバーに接続するとき
    pub ca_file: Option<std::path::PathBuf>,
    pub connect_timeout: Duration,
    /// DB 側の接続一覧に出す名前
    pub application_name: String,
    /// 実際に接続する IP アドレス。SSH トンネル経由のとき `127.0.0.1` を指定する
    /// (`host` は、TLS の証明書の検証に使う名前として、そのまま残す)
    pub hostaddr: Option<std::net::IpAddr>,
}

impl ConnectionConfig {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: None,
            database: None,
            username: None,
            password: None,
            tls: TlsMode::default(),
            ca_file: None,
            connect_timeout: Duration::from_secs(10),
            application_name: "sqltool".into(),
            hostaddr: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_never_appears_in_debug_or_display() {
        let mut cfg = ConnectionConfig::new("db");
        cfg.password = Some(Secret::new("hunter2"));
        assert!(!format!("{cfg:?}").contains("hunter2"));
        assert!(!format!("{:#?}", cfg).contains("hunter2"));
        assert!(!Secret::new("hunter2").to_string().contains("hunter2"));
        assert_eq!(cfg.password.unwrap().expose(), "hunter2");
    }

    #[test]
    fn tls_verifies_by_default() {
        assert_eq!(ConnectionConfig::new("db").tls, TlsMode::Verify);
    }
}
