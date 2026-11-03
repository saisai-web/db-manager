//! トランザクション、キャンセル、一時停止と打ち切り、接続の失敗、バインド。

use std::time::{Duration, Instant};

use driver_api::*;
use driver_sqlserver::{SqlServerDialect, SqlServerDriver};

use crate::common::*;

#[tokio::test]
async fn transactions_belong_to_the_session() {
    let db = ms!();
    let mut a = session(&db).await;
    let mut b = session(&db).await;
    run(&mut a, "CREATE TABLE t (id int)").await;
    run(&mut a, SqlServerDialect.begin_sql()).await;
    run(&mut a, "INSERT INTO t VALUES (1)").await;
    let (_, seen_by_a) = run(&mut a, "SELECT count(*) FROM t").await;
    // 行のロックを待たず、コミットされた行だけを見る
    let (_, seen_by_b) = run(&mut b, "SELECT count(*) FROM t WITH (READPAST)").await;
    assert_eq!(seen_by_a.text(0, 0).as_deref(), Some("1"));
    assert_eq!(
        seen_by_b.text(0, 0).as_deref(),
        Some("0"),
        "uncommitted row must be invisible"
    );
    run(&mut a, SqlServerDialect.rollback_sql()).await;
    let (_, after) = run(&mut a, "SELECT count(*) FROM t").await;
    assert_eq!(after.text(0, 0).as_deref(), Some("0"));
}

#[tokio::test]
async fn ddl_is_transactional() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "BEGIN TRANSACTION").await;
    run(&mut s, "CREATE TABLE gone (id int)").await;
    run(&mut s, "ROLLBACK").await;
    let (_, r) = run(
        &mut s,
        "SELECT count(*) FROM sys.tables WHERE name = 'gone'",
    )
    .await;
    assert_eq!(r.text(0, 0).as_deref(), Some("0"));
}

#[tokio::test]
async fn read_only_is_done_by_the_session_not_by_a_statement() {
    // セッション自身が実現する(`tests/ms/readonly.rs`)。切り替えの SQL は無い
    assert!(SqlServerDriver.dialect().set_read_only_sql(true).is_none());
    assert!(SqlServerDriver.capabilities().read_only_session);
}

#[tokio::test]
async fn cancel_stops_a_waiting_query_quickly() {
    let db = ms!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let started = Instant::now();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("WAITFOR DELAY '00:00:30'"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
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
async fn cancel_stops_a_long_scan_with_a_cancelled_error() {
    let db = ms!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let sql = format!(
        "SELECT SUM(CAST(a.g AS BIGINT) * b.g * c.g) FROM {}, {}, {}",
        numbers_as(4, "a"),
        numbers_as(4, "b"),
        numbers_as(4, "c")
    );
    let exec = tokio::spawn(async move {
        let r = s
            .execute(&Statement::new(sql), &mut Collect::default())
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(800)).await;
    let started = Instant::now();
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec.await.unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled, "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let (_, r) = run(&mut s, "SELECT 'alive'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("alive"));
}

#[tokio::test]
async fn cancel_in_the_middle_of_a_stream_keeps_the_session_usable() {
    let db = ms!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    // 行が出続ける問合せ(結合の直積)。受信の途中で止める
    let sql = format!(
        "SELECT a.g, REPLICATE('x', 50) FROM {}, {}, {}",
        numbers_as(4, "a"),
        numbers_as(4, "b"),
        numbers_as(4, "c")
    );
    let exec = tokio::spawn(async move {
        let r = s
            .execute(&Statement::new(sql), &mut Collect::default())
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec.await.unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled, "{err}");
    let (_, r) = run(&mut s, "SELECT 'alive'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("alive"));
}

#[tokio::test]
async fn pause_resume_and_stop_leave_the_session_usable() {
    let db = ms!();
    let mut s = session(&db).await;
    let mut sink = Collect {
        pause_at: 1000,
        resume_with: vec![Flow::Continue, Flow::Stop],
        ..Collect::default()
    };
    let started = Instant::now();
    let sql = format!(
        "SELECT a.g, REPLICATE('x', 50) FROM {}, {}, {}",
        numbers_as(4, "a"),
        numbers_as(4, "b"),
        numbers_as(4, "c")
    );
    let sum = s.execute(&Statement::new(sql), &mut sink).await.unwrap();
    assert!(sum.stopped_early);
    assert_eq!(sink.resumed, 2);
    assert_eq!(sink.count, 2000);
    assert_eq!(sum.rows_received, 2000);
    assert_eq!(sum.affected_rows, None, "unknown when stopped early");
    // 1 兆行を読み切らずに止められている
    assert!(
        started.elapsed() < Duration::from_secs(15),
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
    let err = SqlServerDriver
        .connect(&cfg)
        .await
        .err()
        .expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
    assert!(!format!("{err:?}").contains("s3cret-pw"));
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let db = ms!();
    let mut cfg = config(&db);
    cfg.password = Some(Secret::new("wrong-pw"));
    let err = SqlServerDriver
        .connect(&cfg)
        .await
        .err()
        .expect("must fail");
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    assert!(!format!("{err:?}").contains("wrong-pw"));
}

#[tokio::test]
async fn bound_parameters_are_applied_exactly_and_safely() {
    let db = ms!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE TABLE t (id bigint PRIMARY KEY, price decimal(20,6), at datetime2(6), note nvarchar(max), bin varbinary(16))",
    )
    .await;
    let d = SqlServerDialect;
    let bind = |n, ty: &str| d.bind_text(n, ty);
    let insert = format!(
        "INSERT INTO t VALUES ({}, {}, {}, {}, {})",
        bind(1, "bigint"),
        bind(2, "decimal(20,6)"),
        bind(3, "datetime2(6)"),
        bind(4, "nvarchar(-1)"),
        bind(5, "varbinary(16)"),
    );
    // 注入になりうる文字列も、値として扱われる。桁数の多い値も桁落ちしない
    let evil = "x'); DROP TABLE t; --";
    let params = vec![
        Some("9007199254740993".to_string()),
        Some("12345678901234.123456".to_string()),
        Some("2026-11-02 12:34:56.123456".to_string()),
        Some(evil.to_string()),
        Some("0x62696e".to_string()),
    ];
    let sum = s
        .execute(
            &Statement::with_params(insert, params),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));

    let null_insert = format!(
        "INSERT INTO t (id, note) VALUES ({}, {})",
        bind(1, "bigint"),
        bind(2, "nvarchar(-1)")
    );
    s.execute(
        &Statement::with_params(null_insert, vec![Some("2".into()), None]),
        &mut Collect::default(),
    )
    .await
    .unwrap();

    let (_, r) = run(
        &mut s,
        "SELECT id, price, at, note, bin FROM t ORDER BY id DESC",
    )
    .await;
    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("9007199254740993"),
        "beyond f64"
    );
    assert_eq!(r.text(0, 1).as_deref(), Some("12345678901234.123456"));
    assert_eq!(r.text(0, 2).as_deref(), Some("2026-11-02 12:34:56.123456"));
    assert_eq!(r.text(0, 3).as_deref(), Some(evil));
    assert_eq!(r.rows[0][4].as_deref(), Some(&b"bin"[..]));
    assert_eq!(
        r.rows[1][3], None,
        "a bound None is NULL, not the string 'NULL'"
    );

    // 桁の大きい整数の比較も、倍精度に丸められない(…992 と …993 は別の行)
    let cond = d.null_safe_equals("note", &bind(3, "nvarchar(-1)"));
    let update = format!(
        "UPDATE t SET note = {} WHERE id = {} AND {cond}",
        bind(1, "nvarchar(-1)"),
        bind(2, "bigint"),
    );
    let run_update = |id: &str| {
        Statement::with_params(
            update.clone(),
            vec![Some("new".into()), Some(id.into()), Some(evil.into())],
        )
    };
    let sum = s
        .execute(&run_update("9007199254740992"), &mut Collect::default())
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(0), "…992 is not …993");
    let sum = s
        .execute(&run_update("9007199254740993"), &mut Collect::default())
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));

    // 型に合わない値は、DB がエラーにする
    let bad = format!("INSERT INTO t (id) VALUES ({})", bind(1, "bigint"));
    let err = s
        .execute(
            &Statement::with_params(bad, vec![Some("not a number".into())]),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query, "{err:?}");
    run(&mut s, "SELECT 1").await;
}

#[tokio::test]
async fn cancelling_reconnects_and_rolls_back_an_open_transaction() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    run(&mut s, "BEGIN TRANSACTION").await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
    let canceller = s.canceller();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("WAITFOR DELAY '00:00:30'"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec.await.unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert!(
        err.hint.is_some(),
        "the user is told the transaction is gone"
    );
    // 張り直した接続では、取り消し済み(サーバーは切断でトランザクションを取り消す)
    let (_, r) = run(&mut s, "SELECT count(*) FROM t").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("0"));
    // 何度止めても、使い続けられる
    for _ in 0..3 {
        let c = s.canceller();
        let exec = tokio::spawn(async move {
            let r = s
                .execute(
                    &Statement::new("WAITFOR DELAY '00:00:30'"),
                    &mut Collect::default(),
                )
                .await;
            (s, r)
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.cancel().await.unwrap();
        let (s2, r) = exec.await.unwrap();
        assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
        s = s2;
        let (_, r) = run(&mut s, "SELECT 'alive'").await;
        assert_eq!(r.text(0, 0).as_deref(), Some("alive"));
    }
}
