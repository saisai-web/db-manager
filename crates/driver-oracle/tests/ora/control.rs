//! トランザクション、キャンセル、一時停止と打ち切り、接続の失敗、バインド。

use std::time::{Duration, Instant};

use driver_api::*;
use driver_oracle::{OracleDialect, OracleDriver};

use crate::common::*;

#[tokio::test]
async fn transactions_belong_to_the_session() {
    let db = ora!();
    let mut a = session(&db).await;
    let mut b = session(&db).await;
    let count = |u: &str| format!("SELECT count(*) FROM \"{u}\".t");
    exec(&mut a, "CREATE TABLE t (id NUMBER)").await;

    // 明示的に始めない限り、1 文ずつ確定する(他の DB と同じ。変更が未コミットのまま残らない)
    exec(&mut a, "INSERT INTO t VALUES (0)").await;
    assert_eq!(
        run(&mut b, &count(&db.user)).await.1.text(0, 0).as_deref(),
        Some("1")
    );

    // 明示的なトランザクション。`begin_sql` はセーブポイントで、いつ実行しても失敗しない
    exec(&mut a, OracleDialect.begin_sql()).await;
    exec(&mut a, OracleDialect.begin_sql()).await;
    exec(&mut a, "INSERT INTO t VALUES (1)").await;
    let seen_by_a = run(&mut a, "SELECT count(*) FROM t").await.1;
    let seen_by_b = run(&mut b, &count(&db.user)).await.1;
    assert_eq!(seen_by_a.text(0, 0).as_deref(), Some("2"));
    assert_eq!(
        seen_by_b.text(0, 0).as_deref(),
        Some("1"),
        "uncommitted row must be invisible"
    );
    exec(&mut a, "ROLLBACK").await;
    assert_eq!(
        run(&mut a, "SELECT count(*) FROM t")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("1")
    );

    exec(&mut a, OracleDialect.begin_sql()).await;
    exec(&mut a, "INSERT INTO t VALUES (2)").await;
    exec(&mut a, "COMMIT").await;
    assert_eq!(
        run(&mut b, &count(&db.user)).await.1.text(0, 0).as_deref(),
        Some("2")
    );
    // 確定した後は、また 1 文ずつ確定する
    exec(&mut a, "INSERT INTO t VALUES (3)").await;
    assert_eq!(
        run(&mut b, &count(&db.user)).await.1.text(0, 0).as_deref(),
        Some("3")
    );
}

/// セッション全体の読み取り専用の SQL はない(ドライバ自身が、文ごとのトランザクションで実現する)。
#[test]
fn read_only_is_not_a_statement_but_a_driver_feature() {
    assert_eq!(OracleDialect.set_read_only_sql(true), None);
    assert!(OracleDriver.capabilities().read_only_session);
}

#[tokio::test]
async fn cancel_without_privileges_is_refused_clearly_and_the_session_survives() {
    let db = ora!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let exec_task = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("BEGIN DBMS_SESSION.SLEEP(3); END;"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let err = canceller.cancel().await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{err:?}");
    assert!(err.message.contains("ALTER SYSTEM"), "{err}");
    let (mut s, result) = exec_task.await.unwrap();
    result.expect("the statement runs to the end");
    run(&mut s, "SELECT 'alive' FROM dual").await;
}

#[tokio::test]
async fn cancel_with_alter_system_stops_the_statement_quickly() {
    let db = ora!();
    test_oracle::admin(&format!("GRANT ALTER SYSTEM TO \"{}\"", db.user)).unwrap();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let started = Instant::now();
    let exec_task = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("BEGIN DBMS_SESSION.SLEEP(60); END;"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec_task.await.unwrap();
    let err = result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled, "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    // セッションごと終了するので、以降の実行は接続が切れたことになる
    let err = s
        .execute(
            &Statement::new("SELECT 1 FROM dual"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Disconnected, "{err:?}");
    // 新しい接続は問題なく使える
    let mut again = session(&db).await;
    run(&mut again, "SELECT 1 FROM dual").await;
}

#[tokio::test]
async fn pause_resume_and_stop_leave_the_session_usable() {
    let db = ora!();
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
            &Statement::new(
                "SELECT level, rpad('x', 50, 'x') FROM dual CONNECT BY level <= 50000000",
            ),
            &mut sink,
        )
        .await
        .unwrap();
    assert!(sum.stopped_early);
    assert_eq!(sink.resumed, 2);
    assert_eq!(sink.count, 2000);
    assert_eq!(sum.rows_received, 2000);
    assert_eq!(sum.affected_rows, None, "unknown when stopped early");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    let (_, r) = run(&mut s, "SELECT 'next' FROM dual").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("next"));
}

#[tokio::test]
async fn connection_failure_is_reported_without_leaking_the_password() {
    let mut cfg = ConnectionConfig::new("127.0.0.1");
    cfg.port = Some(1); // 誰も待ち受けていない
    cfg.database = Some("FREEPDB1".into());
    cfg.tls = TlsMode::Disable;
    cfg.password = Some(Secret::new("s3cret-pw"));
    cfg.connect_timeout = Duration::from_secs(3);
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
    assert!(!format!("{err:?}").contains("s3cret-pw"));
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let db = ora!();
    let mut cfg = config(&db);
    cfg.password = Some(Secret::new("wrong-pw"));
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    assert!(!format!("{err:?}").contains("wrong-pw"));
    // サービス名の指定がなければ、接続前に分かりやすく失敗する
    let mut cfg = config(&db);
    cfg.database = None;
    let err = OracleDriver.connect(&cfg).await.err().expect("must fail");
    assert!(err.message.contains("サービス名"), "{err}");
}

#[tokio::test]
async fn bound_parameters_are_applied_exactly_and_safely() {
    let db = ora!();
    let mut s = session(&db).await;
    exec(
        &mut s,
        "CREATE TABLE t (id NUMBER(38) PRIMARY KEY, price NUMBER(20,6), d DATE, ts TIMESTAMP(6), note VARCHAR2(100), bin RAW(16))",
    )
    .await;
    let d = OracleDialect;
    let bind = |n, ty: &str| d.bind_text(n, ty);
    let insert = format!(
        "INSERT INTO t VALUES ({}, {}, {}, {}, {}, {})",
        bind(1, "NUMBER(38)"),
        bind(2, "NUMBER(20,6)"),
        bind(3, "DATE"),
        bind(4, "TIMESTAMP(6)"),
        bind(5, "VARCHAR2(100)"),
        bind(6, "RAW(16)"),
    );
    // 注入になりうる文字列も、値として扱われる。桁数の多い値も桁落ちしない
    let evil = "x'); DROP TABLE t; --";
    let params = vec![
        Some("9007199254740993".to_string()),
        Some("12345678901234.123456".to_string()),
        Some("2026-11-02 12:34:56".to_string()),
        Some("2026-11-02 12:34:56.123456".to_string()),
        Some(evil.to_string()),
        Some("DEADBEEF".to_string()),
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
        bind(1, "NUMBER(38)"),
        bind(2, "VARCHAR2(100)")
    );
    s.execute(
        &Statement::with_params(null_insert, vec![Some("2".into()), None]),
        &mut Collect::default(),
    )
    .await
    .unwrap();

    let (_, r) = run(
        &mut s,
        "SELECT id, price, d, ts, note, RAWTOHEX(bin) FROM t ORDER BY id DESC",
    )
    .await;
    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("9007199254740993"),
        "beyond f64"
    );
    assert_eq!(r.text(0, 1).as_deref(), Some("12345678901234.123456"));
    assert_eq!(r.text(0, 2).as_deref(), Some("2026-11-02 12:34:56"));
    assert_eq!(r.text(0, 3).as_deref(), Some("2026-11-02 12:34:56.123456"));
    assert_eq!(r.text(0, 4).as_deref(), Some(evil));
    assert_eq!(r.text(0, 5).as_deref(), Some("DEADBEEF"));
    assert_eq!(
        r.rows[1][4], None,
        "a bound None is NULL, not the string 'NULL'"
    );

    // 桁の大きい整数の比較も、丸められない(…992 と …993 は別の行)。NULL どうしも等しいとみなす
    let update = format!(
        "UPDATE t SET note = {} WHERE id = {} AND {}",
        bind(1, "VARCHAR2(100)"),
        bind(2, "NUMBER(38)"),
        d.null_safe_equals("note", &bind(3, "VARCHAR2(100)"))
    );
    let affected = |id: &str, old: Option<&str>| {
        Statement::with_params(
            update.clone(),
            vec![Some("new".into()), Some(id.into()), old.map(String::from)],
        )
    };
    let sum = s
        .execute(
            &affected("9007199254740992", Some(evil)),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(0), "…992 is not …993");
    let sum = s
        .execute(
            &affected("9007199254740993", Some(evil)),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));
    let sum = s
        .execute(&affected("2", None), &mut Collect::default())
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1), "NULL matches NULL");

    // 型に合わない値は、DB がエラーにする
    let bad = format!("INSERT INTO t (id) VALUES ({})", bind(1, "NUMBER(38)"));
    let err = s
        .execute(
            &Statement::with_params(bad, vec![Some("not a number".into())]),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query, "{err:?}");
    run(&mut s, "SELECT 1 FROM dual").await;
}

/// 画面に出した日時の文字列を、そのまま書き戻せる(小数の桁数が違っても、UTC 付きでも)。
#[tokio::test]
async fn displayed_timestamps_can_be_written_back() {
    let db = ora!();
    let mut s = session(&db).await;
    exec(
        &mut s,
        "CREATE TABLE t (id NUMBER PRIMARY KEY, t0 TIMESTAMP(0), t3 TIMESTAMP(3), tz TIMESTAMP(6) WITH TIME ZONE)",
    )
    .await;
    let d = OracleDialect;
    let insert = format!(
        "INSERT INTO t VALUES (1, {}, {}, {})",
        d.bind_text(1, "TIMESTAMP(0)"),
        d.bind_text(2, "TIMESTAMP(3)"),
        d.bind_text(3, "TIMESTAMP(6) WITH TIME ZONE"),
    );
    let params = vec![
        Some("2026-11-02 12:34:56".to_string()),
        Some("2026-11-02 12:34:56.789".to_string()),
        Some("2026-11-02 03:34:56.500000 UTC".to_string()),
    ];
    s.execute(
        &Statement::with_params(insert, params),
        &mut Collect::default(),
    )
    .await
    .unwrap();
    let (_, r) = run(&mut s, "SELECT t0, t3, tz FROM t").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("2026-11-02 12:34:56"));
    assert_eq!(r.text(0, 1).as_deref(), Some("2026-11-02 12:34:56.789"));
    assert_eq!(
        r.text(0, 2).as_deref(),
        Some("2026-11-02 03:34:56.500000 UTC")
    );
}
