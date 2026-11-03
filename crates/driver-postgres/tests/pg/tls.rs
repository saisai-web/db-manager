//! TLS 接続: 暗号化、証明書の検証、検証なしの接続、停止要求。

use std::time::Duration;

use driver_api::{Driver as _, ErrorKind, Session, Statement, TlsMode};
use driver_postgres::PostgresDriver;

use crate::common::*;

/// TLS を試せるサーバー(自己署名証明書つき)があるときだけ、そのテストを行う。
macro_rules! tls_db {
    () => {{
        let db = pg!();
        if db.ssl_cert.is_none() {
            eprintln!("SKIP: TLS 対応のテスト用サーバーがありません");
            return;
        }
        db
    }};
}

async fn is_encrypted(s: &mut Box<dyn Session>) -> bool {
    let (_, r) = run(
        s,
        "select ssl from pg_stat_ssl where pid = pg_backend_pid()",
    )
    .await;
    r.text(0, 0).as_deref() == Some("t")
}

#[tokio::test]
async fn an_unverified_tls_connection_is_encrypted() {
    let db = tls_db!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::SkipVerify;
    let mut s = PostgresDriver.connect(&cfg).await.expect("connect");
    assert!(
        is_encrypted(&mut s).await,
        "the session must really be encrypted"
    );
    let (_, r) = run(&mut s, "select 'over tls'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("over tls"));
}

#[tokio::test]
async fn verification_rejects_a_certificate_nobody_vouches_for() {
    let db = tls_db!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::Verify; // 既定。自己署名の証明書は、信頼されない
    let err = PostgresDriver
        .connect(&cfg)
        .await
        .err()
        .expect("must be rejected");
    assert_eq!(err.kind, ErrorKind::Connect);
    assert!(
        err.message.to_lowercase().contains("certificate") || err.message.contains("UnknownIssuer"),
        "the reason should be visible: {}",
        err.message
    );
}

#[tokio::test]
async fn verification_succeeds_when_the_ca_is_provided() {
    let db = tls_db!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::Verify;
    cfg.ca_file = db.ssl_cert.clone();
    let mut s = PostgresDriver
        .connect(&cfg)
        .await
        .expect("connect with the CA");
    assert!(is_encrypted(&mut s).await);
}

#[tokio::test]
async fn a_missing_or_invalid_ca_file_is_reported_clearly() {
    let db = tls_db!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::Verify;
    cfg.ca_file = Some("/nonexistent/ca.pem".into());
    let err = PostgresDriver.connect(&cfg).await.err().expect("must fail");
    assert!(err.message.contains("CA 証明書"), "{}", err.message);
}

#[tokio::test]
async fn cancel_and_metadata_work_over_tls() {
    let db = tls_db!();
    let mut cfg = config(&db);
    cfg.tls = TlsMode::SkipVerify;
    let mut s = PostgresDriver.connect(&cfg).await.unwrap();
    let canceller = s.canceller();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("select pg_sleep(30)"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.expect("cancel over TLS");
    let (mut s, r) = tokio::time::timeout(Duration::from_secs(10), exec)
        .await
        .expect("not hung")
        .unwrap();
    assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
    assert!(is_encrypted(&mut s).await);

    let meta = PostgresDriver
        .connect_metadata(&cfg)
        .await
        .expect("metadata over TLS");
    assert!(!meta.schemas().await.unwrap().is_empty());
}
