//! トランザクション、読み取り専用、キャンセル、一時停止と打ち切り、接続の失敗。

use std::time::{Duration, Instant};

use driver_api::*;
use driver_mysql::{MysqlDialect, MysqlDriver};

use crate::common::*;

#[tokio::test]
async fn transactions_belong_to_the_session() {
    let db = my!();
    let mut a = session(&db).await;
    let mut b = session(&db).await;
    run(&mut a, "CREATE TABLE t (id int)").await;
    run(&mut a, MysqlDialect.begin_sql()).await;
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
    let (_, after) = run(&mut a, "SELECT count(*) FROM t").await;
    assert_eq!(after.text(0, 0).as_deref(), Some("0"));
}

#[tokio::test]
async fn read_only_session_rejects_writes() {
    let db = my!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    let dialect = MysqlDialect;
    run(&mut s, &dialect.set_read_only_sql(true).unwrap()).await;
    let err = s
        .execute(
            &Statement::new("INSERT INTO t VALUES (1)"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.code.as_deref().is_some_and(|c| c.starts_with("1792")),
        "{err:?}"
    );
    run(&mut s, "SELECT * FROM t").await;
    run(&mut s, &dialect.set_read_only_sql(false).unwrap()).await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
}

#[tokio::test]
async fn cancel_stops_a_running_query_quickly() {
    let db = my!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let started = Instant::now();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(&Statement::new("SELECT SLEEP(30)"), &mut Collect::default())
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.unwrap();
    let (mut s, result) = exec.await.unwrap();
    // SLEEP は中断されると 1 を返して正常終了する。長い SELECT は 1317 で失敗する
    match result {
        Ok(_) => {}
        Err(e) => assert_eq!(e.kind, ErrorKind::Cancelled, "{e}"),
    }
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
    let db = my!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE big (id int PRIMARY KEY)").await;
    run(
        &mut s,
        &format!(
            "INSERT INTO big SELECT g + 1 FROM {} WHERE g < 2000",
            numbers(4)
        ),
    )
    .await;
    let canceller = s.canceller();
    let exec = tokio::spawn(async move {
        // 2000^3 行の結合。止めなければ終わらない
        let r = s
            .execute(
                &Statement::new("SELECT SUM(a.id * b.id * c.id) FROM big a, big b, big c"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
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
async fn pause_resume_and_stop_leave_the_session_usable() {
    let db = my!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE big (id int PRIMARY KEY)").await;
    run(
        &mut s,
        &format!(
            "INSERT INTO big SELECT g + 1 FROM {} WHERE g < 5000",
            numbers(4)
        ),
    )
    .await;

    // 一時停止 → 続行 → 一時停止 → 打ち切り
    let mut sink = Collect {
        pause_at: 1000,
        resume_with: vec![Flow::Continue, Flow::Stop],
        ..Collect::default()
    };
    let started = Instant::now();
    let sum = s
        .execute(
            &Statement::new("SELECT a.id, repeat('x', 50) FROM big a, big b, big c"),
            &mut sink,
        )
        .await
        .unwrap();
    assert!(sum.stopped_early);
    assert_eq!(sink.resumed, 2);
    assert_eq!(sink.count, 2000);
    assert_eq!(sum.rows_received, 2000);
    assert_eq!(sum.affected_rows, None, "unknown when stopped early");
    // 1250 億行を読み切らずに止められている
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
    let err = MysqlDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
    assert!(!format!("{err:?}").contains("s3cret-pw"));
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let db = my!();
    // サーバーを使い回す環境でも重ならないよう、利用者名はテストごとに変える
    let user = format!("u_{}", db.dbname);
    run(
        &mut session(&db).await,
        &format!("CREATE USER '{user}'@'%' IDENTIFIED BY 'right-pw'"),
    )
    .await;
    run(
        &mut session(&db).await,
        &format!("GRANT ALL ON `{}`.* TO '{user}'@'%'", db.dbname),
    )
    .await;
    let mut cfg = config(&db);
    cfg.username = Some(user.clone());
    cfg.password = Some(Secret::new("wrong-pw"));
    let err = MysqlDriver.connect(&cfg).await.err().expect("must fail");
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    assert!(!format!("{err:?}").contains("wrong-pw"));
    cfg.password = Some(Secret::new("right-pw"));
    MysqlDriver
        .connect(&cfg)
        .await
        .expect("the right password works");
}

#[tokio::test]
async fn bound_parameters_are_applied_exactly_and_safely() {
    let db = my!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE TABLE t (id bigint PRIMARY KEY, price decimal(20,6), at datetime(6), note text, bin varbinary(16))",
    )
    .await;
    let d = MysqlDialect;
    let bind = |n, ty: &str| d.bind_text(n, ty);
    let insert = format!(
        "INSERT INTO t VALUES ({}, {}, {}, {}, {})",
        bind(1, "bigint"),
        bind(2, "decimal(20,6)"),
        bind(3, "datetime(6)"),
        bind(4, "text"),
        bind(5, "varbinary(16)"),
    );
    // 注入になりうる文字列も、値として扱われる。桁数の多い値も桁落ちしない
    let evil = "x'); DROP TABLE t; --";
    let params = vec![
        Some("9007199254740993".to_string()),
        Some("12345678901234.123456".to_string()),
        Some("2026-11-02 12:34:56.123456".to_string()),
        Some(evil.to_string()),
        Some("bin".to_string()),
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
        bind(2, "text")
    );
    s.execute(
        &Statement::with_params(null_insert, vec![Some("2".into()), None]),
        &mut Collect::default(),
    )
    .await
    .unwrap();

    let (_, r) = run(&mut s, "SELECT id, price, at, note FROM t ORDER BY id DESC").await;
    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("9007199254740993"),
        "beyond f64"
    );
    assert_eq!(r.text(0, 1).as_deref(), Some("12345678901234.123456"));
    assert_eq!(r.text(0, 2).as_deref(), Some("2026-11-02 12:34:56.123456"));
    assert_eq!(r.text(0, 3).as_deref(), Some(evil));
    assert_eq!(
        r.rows[1][3], None,
        "a bound None is NULL, not the string 'NULL'"
    );

    // 桁の大きい整数の比較も、倍精度に丸められない(…992 と …993 は別の行)
    let update = format!(
        "UPDATE t SET note = {} WHERE id = {} AND note {}",
        bind(1, "text"),
        bind(2, "bigint"),
        d.null_safe_equals("", &bind(3, "text")).trim_start()
    );
    let sum = s
        .execute(
            &Statement::with_params(
                update.clone(),
                vec![
                    Some("new".into()),
                    Some("9007199254740992".into()),
                    Some(evil.into()),
                ],
            ),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(0), "…992 is not …993");
    let sum = s
        .execute(
            &Statement::with_params(
                update,
                vec![
                    Some("new".into()),
                    Some("9007199254740993".into()),
                    Some(evil.into()),
                ],
            ),
            &mut Collect::default(),
        )
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
        .await;
    // MySQL は厳格モードでなければ 0 に丸める。どちらでも、セッションは使える
    let _ = err;
    run(&mut s, "SELECT 1").await;
}
