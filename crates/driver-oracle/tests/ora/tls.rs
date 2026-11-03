//! TLS: 対応しない指定は、平文に落とさず、明示的に拒否する。

use driver_api::*;
use driver_oracle::OracleDriver;

use crate::common::*;

#[tokio::test]
async fn unsupported_tls_modes_are_refused_not_downgraded() {
    let db = ora!();
    let mut cfg = config(&db);

    cfg.tls = TlsMode::SkipVerify;
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err:?}");

    cfg.tls = TlsMode::Verify;
    cfg.hostaddr = Some("127.0.0.1".parse().unwrap());
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err:?}");
}

#[tokio::test]
async fn verified_tls_never_falls_back_to_plaintext() {
    let db = ora!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::Verify; // tcps で接続する。テスト用のサーバーは TLS を待ち受けていない
    cfg.connect_timeout = std::time::Duration::from_secs(5);
    let err = OracleDriver
        .connect(&cfg)
        .await
        .err()
        .expect("must fail, not connect in plaintext");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}

/// 実際の TLS: テスト用の CA で署名した証明書を持つ中継(Oracle の手前)に、`ca_file` を渡して接続する。
#[tokio::test]
async fn verified_tls_with_a_ca_file_reaches_a_real_oracle() {
    let db = ora!();
    let Some(proxy) = test_oracle::TlsProxy::start(&db.host, db.port).await else {
        eprintln!("openssl が使えないため、TLS の結合テストを飛ばします");
        return;
    };
    let mut cfg = config(&db);
    cfg.host = "localhost".into();
    cfg.port = Some(proxy.port);
    cfg.tls = TlsMode::Verify;
    cfg.connect_timeout = std::time::Duration::from_secs(10);

    // CA を渡さなければ、組み込みの CA では信頼できず、拒否される(検証が働いている)
    let err = OracleDriver.connect(&cfg).await.err().expect("untrusted");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");

    // 渡すと、TLS で接続できて、クエリが動く
    cfg.ca_file = Some(proxy.ca_file.clone());
    let mut session = OracleDriver.connect(&cfg).await.expect("trusted CA");
    let mut sink = Collect::keeping();
    session
        .execute(&Statement::new("SELECT 1 FROM dual"), &mut sink)
        .await
        .unwrap();
    assert_eq!(sink.text(0, 0).as_deref(), Some("1"));

    // 別の CA を渡すと、拒否される
    let other = test_oracle::TlsProxy::start(&db.host, db.port)
        .await
        .unwrap();
    cfg.ca_file = Some(other.ca_file.clone());
    let err = OracleDriver.connect(&cfg).await.err().expect("wrong CA");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}

#[tokio::test]
async fn a_missing_or_malformed_ca_file_is_a_connect_error() {
    let db = ora!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::Verify;
    cfg.ca_file = Some("/nonexistent/ca.pem".into());
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}
