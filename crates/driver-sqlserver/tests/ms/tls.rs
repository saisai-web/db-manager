//! TLS: 暗号化の有無と、証明書の検証。

use driver_api::*;
use driver_sqlserver::SqlServerDriver;

use crate::common::*;

async fn encrypted(cfg: &ConnectionConfig) -> Result<bool> {
    let mut s = SqlServerDriver.connect(cfg).await?;
    let (_, r) = run(
        &mut s,
        "SELECT encrypt_option FROM sys.dm_exec_connections WHERE session_id = @@SPID",
    )
    .await;
    Ok(r.text(0, 0).as_deref() == Some("TRUE"))
}

#[tokio::test]
async fn skip_verify_encrypts_and_verify_refuses_an_untrusted_certificate() {
    let db = ms!();
    let mut cfg = config(&db);

    cfg.tls = TlsMode::Disable;
    assert!(
        !encrypted(&cfg).await.unwrap(),
        "no encryption when disabled"
    );

    cfg.tls = TlsMode::SkipVerify;
    assert!(
        encrypted(&cfg).await.unwrap(),
        "the session must be encrypted"
    );

    // 既定は検証する。サーバーの自己署名証明書は、OS も信頼しない
    cfg.tls = TlsMode::Verify;
    let err = encrypted(&cfg).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}
