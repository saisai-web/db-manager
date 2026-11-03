//! TLS: 暗号化の有無と、証明書の検証。サーバーが TLS に対応していなければ飛ばす。

use driver_api::*;
use driver_mysql::MysqlDriver;

use crate::common::*;

async fn cipher(cfg: &ConnectionConfig) -> Result<String> {
    let mut s = MysqlDriver.connect(cfg).await?;
    let (_, r) = run(&mut s, "SHOW SESSION STATUS LIKE 'Ssl_cipher'").await;
    Ok(r.text(0, 1).unwrap_or_default())
}

#[tokio::test]
async fn skip_verify_encrypts_and_verify_refuses_an_untrusted_certificate() {
    let db = my!();
    let mut cfg = config(&db);

    cfg.tls = TlsMode::Disable;
    assert_eq!(
        cipher(&cfg).await.unwrap(),
        "",
        "no encryption when disabled"
    );

    cfg.tls = TlsMode::SkipVerify;
    match cipher(&cfg).await {
        Ok(c) => assert!(!c.is_empty(), "the session must be encrypted"),
        Err(e) => {
            eprintln!("SKIP: このサーバーは TLS に対応していません: {e}");
            return;
        }
    }

    // 既定は検証する。サーバーの自己署名証明書は、OS も webpki も信頼しない
    cfg.tls = TlsMode::Verify;
    let err = cipher(&cfg).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}
