//! 値の正確さ、DML の影響行数、エラー、大量行の受信。

use std::time::Instant;

use driver_api::*;

use crate::common::*;

#[tokio::test]
async fn connects_and_reports_server_version() {
    let db = pg!();
    let s = session(&db).await;
    assert!(
        s.server_info().version.starts_with("PostgreSQL"),
        "{}",
        s.server_info().version
    );
    s.close().await.unwrap();
}

#[tokio::test]
async fn values_come_back_exactly_as_the_server_formats_them() {
    let db = pg!();
    let mut s = session(&db).await;
    run(&mut s, "SET TimeZone = 'Asia/Tokyo'").await;
    let (sum, r) = run(
        &mut s,
        "SELECT 12345678901234567890.123456789012345678::numeric AS big,
                9223372036854775807::bigint AS maxint,
                0.1::float8 + 0.2::float8 AS f,
                '2026-11-02 12:34:56.123456+09'::timestamptz AS ts,
                '日本語のテキスト'::text AS jp,
                NULL::text AS nul,
                ''::text AS empty,
                true AS b,
                '\\x00ff10'::bytea AS bin,
                'a\"b'::text AS quote,
                1.50::numeric AS trailing",
    )
    .await;
    assert_eq!(sum.rows_received, 1);
    assert_eq!(sum.affected_rows, Some(1));
    assert!(!sum.stopped_early);

    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("12345678901234567890.123456789012345678")
    );
    assert_eq!(r.text(0, 1).as_deref(), Some("9223372036854775807"));
    assert_eq!(r.text(0, 2).as_deref(), Some("0.30000000000000004"));
    assert_eq!(
        r.text(0, 3).as_deref(),
        Some("2026-11-02 12:34:56.123456+09")
    );
    assert_eq!(r.text(0, 4).as_deref(), Some("日本語のテキスト"));
    assert_eq!(r.rows[0][5], None, "NULL");
    assert_eq!(
        r.rows[0][6].as_deref(),
        Some(&b""[..]),
        "empty string is not NULL"
    );
    assert_eq!(r.text(0, 7).as_deref(), Some("t"));
    assert_eq!(
        r.rows[0][8].as_deref(),
        Some(&[0x00, 0xff, 0x10][..]),
        "bytea is decoded to bytes"
    );
    assert_eq!(r.text(0, 9).as_deref(), Some("a\"b"));
    assert_eq!(r.text(0, 10).as_deref(), Some("1.50"), "scale is preserved");

    let classes: Vec<_> = r.columns.iter().map(|c| c.class).collect();
    assert_eq!(
        classes,
        [
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Temporal,
            ValueClass::Text,
            ValueClass::Text,
            ValueClass::Text,
            ValueClass::Bool,
            ValueClass::Binary,
            ValueClass::Text,
            ValueClass::Number,
        ]
    );
    assert_eq!(r.columns[0].name, "big");
    assert_eq!(r.columns[0].type_name.as_deref(), Some("numeric"));
}

#[tokio::test]
async fn dml_reports_affected_rows_and_ddl_returns_no_result_set() {
    let db = pg!();
    let mut s = session(&db).await;
    let (sum, r) = run(&mut s, "CREATE TABLE t (id int PRIMARY KEY, v text)").await;
    assert!(r.columns.is_empty());
    assert_eq!(sum.rows_received, 0);

    let (sum, r) = run(
        &mut s,
        "INSERT INTO t SELECT g, 'x' FROM generate_series(1, 5) g",
    )
    .await;
    assert!(r.columns.is_empty(), "INSERT has no result set");
    assert_eq!(sum.affected_rows, Some(5));

    let (sum, _) = run(&mut s, "UPDATE t SET v = 'y' WHERE id <= 2").await;
    assert_eq!(sum.affected_rows, Some(2));
    let (sum, _) = run(&mut s, "DELETE FROM t WHERE id = 99").await;
    assert_eq!(sum.affected_rows, Some(0));

    let (sum, r) = run(&mut s, "INSERT INTO t VALUES (10, 'z') RETURNING id").await;
    assert_eq!(r.rows.len(), 1);
    assert_eq!(sum.affected_rows, Some(1));
}

#[tokio::test]
async fn syntax_error_reports_the_character_position_not_the_byte_position() {
    let db = pg!();
    let mut s = session(&db).await;
    // 先頭に多バイト文字があっても、位置は文字数で数える
    let sql = "SELECT '日本語', FROM t";
    let err = s
        .execute(&Statement::new(sql), &mut Collect::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert_eq!(err.code.as_deref(), Some("42601"));
    let pos = err.position.expect("position");
    let culprit: String = sql.chars().skip(pos).take(4).collect();
    assert_eq!(
        culprit, "FROM",
        "position {pos} points at the offending token"
    );
}

#[tokio::test]
async fn runtime_error_carries_code_and_session_survives() {
    let db = pg!();
    let mut s = session(&db).await;
    let err = s
        .execute(&Statement::new("SELECT 1/0"), &mut Collect::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert_eq!(err.code.as_deref(), Some("22012"));
    let (_, r) = run(&mut s, "SELECT 1").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("1"));
}

#[tokio::test]
async fn a_million_rows_stream_through() {
    let db = pg!();
    let mut s = session(&db).await;
    let mut sink = Collect::default();
    let started = Instant::now();
    let sum = s
        .execute(
            &Statement::new(
                "SELECT g, g::numeric / 7, 'row ' || g, g % 2 = 0, NULL FROM generate_series(1, 1000000) g",
            ),
            &mut sink,
        )
        .await
        .unwrap();
    eprintln!("1M rows x 5 cols: {:?}", started.elapsed());
    assert_eq!(sum.rows_received, 1_000_000);
    assert_eq!(sum.affected_rows, Some(1_000_000));
    assert_eq!(sink.count, 1_000_000);
}
