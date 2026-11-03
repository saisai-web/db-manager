//! トランザクション、読み取り専用、キャンセル、一時停止と打ち切り、接続の失敗。

use std::time::{Duration, Instant};

use driver_api::*;
use driver_postgres::{PostgresDialect, PostgresDriver};

use crate::common::*;

#[tokio::test]
async fn transactions_belong_to_the_session() {
    let db = pg!();
    let mut a = session(&db).await;
    let mut b = session(&db).await;
    run(&mut a, "CREATE TABLE t (id int)").await;
    run(&mut a, "BEGIN").await;
    run(&mut a, "INSERT INTO t VALUES (1)").await;
    let (_, seen_by_a) = run(&mut a, "SELECT count(*) FROM t").await;
    let (_, seen_by_b) = run(&mut b, "SELECT count(*) FROM t").await;
    assert_eq!(seen_by_a.text(0, 0).as_deref(), Some("1"));
    assert_eq!(
        seen_by_b.text(0, 0).as_deref(),
        Some("0"),
        "uncommitted row must be invisible"
    );
    run(&mut a, "ROLLBACK").await;
}

#[tokio::test]
async fn failed_transaction_blocks_statements_until_rollback() {
    let db = pg!();
    let mut s = session(&db).await;
    run(&mut s, "BEGIN").await;
    assert!(
        s.execute(&Statement::new("SELECT 1/0"), &mut Collect::default())
            .await
            .is_err()
    );
    let err = s
        .execute(&Statement::new("SELECT 1"), &mut Collect::default())
        .await
        .unwrap_err();
    assert_eq!(err.code.as_deref(), Some("25P02"));
    run(&mut s, "ROLLBACK").await;
    run(&mut s, "SELECT 1").await;
}

#[tokio::test]
async fn read_only_session_rejects_writes() {
    let db = pg!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    let dialect = PostgresDialect;
    run(&mut s, &dialect.set_read_only_sql(true).unwrap()).await;
    let err = s
        .execute(
            &Statement::new("INSERT INTO t VALUES (1)"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code.as_deref(), Some("25006"));
    run(&mut s, "SELECT * FROM t").await;
    run(&mut s, &dialect.set_read_only_sql(false).unwrap()).await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
}

#[tokio::test]
async fn cancel_stops_a_running_query_quickly() {
    let db = pg!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let started = Instant::now();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("SELECT pg_sleep(30)"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec.await.unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled, "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    // キャンセル後もセッションは使える
    let (_, r) = run(&mut s, "SELECT 'alive'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("alive"));
}

#[tokio::test]
async fn pause_resume_and_stop_leave_the_session_usable() {
    let db = pg!();
    let mut s = session(&db).await;

    // 一時停止 → 続行 → 一時停止 → 打ち切り
    let mut sink = Collect {
        pause_at: 1000,
        resume_with: vec![Flow::Continue, Flow::Stop],
        ..Collect::default()
    };
    let started = Instant::now();
    let sum = s
        .execute(
            &Statement::new("SELECT g, repeat('x', 50) FROM generate_series(1, 50000000) g"),
            &mut sink,
        )
        .await
        .unwrap();
    assert!(sum.stopped_early);
    assert_eq!(sink.resumed, 2);
    assert_eq!(sink.count, 2000);
    assert_eq!(sum.rows_received, 2000);
    assert_eq!(sum.affected_rows, None, "unknown when stopped early");
    // 5000 万行を読み切らずに止められている
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );

    let (_, r) = run(&mut s, "SELECT 'next'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("next"));
}

#[tokio::test]
async fn connection_failure_is_reported_without_leaking_the_password() {
    let mut cfg = ConnectionConfig::new("127.0.0.1");
    cfg.port = Some(1); // 誰も待ち受けていない
    cfg.tls = TlsMode::Disable;
    cfg.password = Some(Secret::new("s3cret-pw"));
    cfg.connect_timeout = Duration::from_secs(3);
    let err = PostgresDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
    assert!(!format!("{err:?}").contains("s3cret-pw"));
}

#[tokio::test]
async fn bound_parameters_are_applied_exactly_and_safely() {
    let db = pg!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE TABLE t (id bigint PRIMARY KEY, price numeric(20,6), at timestamptz, note text, bin bytea, ok boolean)",
    )
    .await;
    let d = PostgresDialect;
    let bind = |n, ty: &str| d.bind_text(n, ty);
    let insert = format!(
        "INSERT INTO t VALUES ({}, {}, {}, {}, {}, {})",
        bind(1, "bigint"),
        bind(2, "numeric(20,6)"),
        bind(3, "timestamp with time zone"),
        bind(4, "text"),
        bind(5, "bytea"),
        bind(6, "boolean"),
    );
    // 注入になりうる文字列も、値として扱われる。桁数の多い numeric も桁落ちしない
    let evil = "x'); DROP TABLE t; --";
    let params = vec![
        Some("9007199254740993".to_string()),
        Some("12345678901234.123456".to_string()),
        Some("2026-11-02 12:34:56.123456+09".to_string()),
        Some(evil.to_string()),
        Some("\\xdeadbeef".to_string()),
        Some("true".to_string()),
    ];
    let mut sink = Collect::default();
    let sum = s
        .execute(&Statement::with_params(insert, params), &mut sink)
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));

    let null_insert = format!(
        "INSERT INTO t (id, note) VALUES ({}, {})",
        bind(1, "bigint"),
        bind(2, "text")
    );
    s.execute(
        &Statement::with_params(null_insert, vec![Some("2".into()), None]),
        &mut Collect::default(),
    )
    .await
    .unwrap();

    run(&mut s, "SET TimeZone = 'Asia/Tokyo'").await;
    let (_, r) = run(
        &mut s,
        "SELECT id, price, at, note, encode(bin, 'hex'), ok FROM t ORDER BY id DESC",
    )
    .await;
    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("9007199254740993"),
        "beyond f64's exact integers"
    );
    assert_eq!(r.text(0, 1).as_deref(), Some("12345678901234.123456"));
    assert_eq!(
        r.text(0, 2).as_deref(),
        Some("2026-11-02 12:34:56.123456+09")
    );
    assert_eq!(r.text(0, 3).as_deref(), Some(evil));
    assert_eq!(r.text(0, 4).as_deref(), Some("deadbeef"));
    assert_eq!(
        r.rows[1][3], None,
        "a bound None is NULL, not the string 'NULL'"
    );

    // 条件に合わなければ、影響行数は 0
    let update = format!(
        "UPDATE t SET note = {} WHERE id = {} AND note IS NOT DISTINCT FROM {}",
        bind(1, "text"),
        bind(2, "bigint"),
        bind(3, "text")
    );
    let sum = s
        .execute(
            &Statement::with_params(
                update,
                vec![Some("new".into()), Some("2".into()), Some("stale".into())],
            ),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(0));

    // 型に合わない値は、DB がエラーにする(位置情報なしの Query エラー)
    let bad = format!("INSERT INTO t (id) VALUES ({})", bind(1, "bigint"));
    let err = s
        .execute(
            &Statement::with_params(bad, vec![Some("not a number".into())]),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    run(&mut s, "SELECT 1").await;
}
