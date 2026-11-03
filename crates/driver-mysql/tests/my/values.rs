//! 値の正確さ、DML の影響行数、エラー、大量行の受信。

use std::time::Instant;

use driver_api::*;

use crate::common::*;

#[tokio::test]
async fn connects_and_reports_server_version() {
    let db = my!();
    let s = session(&db).await;
    assert!(
        s.server_info()
            .version
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "{}",
        s.server_info().version
    );
    s.close().await.unwrap();
}

#[tokio::test]
async fn values_come_back_exactly_as_the_server_formats_them() {
    let db = my!();
    let mut s = session(&db).await;
    let (sum, r) = run(
        &mut s,
        "SELECT CAST('12345678901234567890.123456789012345678' AS DECIMAL(40,18)) AS dec_big,
                9223372036854775807 AS maxint,
                CAST(18446744073709551615 AS UNSIGNED) AS maxu,
                CAST('2026-11-02 12:34:56.123456' AS DATETIME(6)) AS ts,
                '日本語のテキスト' AS jp,
                NULL AS nul,
                '' AS emp,
                x'00ff10' AS bin,
                'a\"b' AS quote,
                CAST(1.5 AS DECIMAL(5,2)) AS trl,
                CAST('10:20:30.5' AS TIME(3)) AS t",
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
    assert_eq!(r.text(0, 2).as_deref(), Some("18446744073709551615"));
    assert_eq!(
        r.text(0, 3).as_deref(),
        Some("2026-11-02 12:34:56.123456"),
        "datetimes keep the server's text"
    );
    assert_eq!(r.text(0, 4).as_deref(), Some("日本語のテキスト"));
    assert_eq!(r.rows[0][5], None, "NULL");
    assert_eq!(
        r.rows[0][6].as_deref(),
        Some(&b""[..]),
        "empty string is not NULL"
    );
    assert_eq!(r.rows[0][7].as_deref(), Some(&[0x00, 0xff, 0x10][..]));
    assert_eq!(r.text(0, 8).as_deref(), Some("a\"b"));
    assert_eq!(r.text(0, 9).as_deref(), Some("1.50"), "scale is preserved");
    assert_eq!(r.text(0, 10).as_deref(), Some("10:20:30.500"));

    let classes: Vec<_> = r.columns.iter().map(|c| c.class).collect();
    assert_eq!(
        classes,
        [
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Temporal,
            ValueClass::Text,
            ValueClass::Other, // NULL リテラルの型
            ValueClass::Text,
            ValueClass::Binary,
            ValueClass::Text,
            ValueClass::Number,
            ValueClass::Temporal,
        ]
    );
    assert_eq!(r.columns[0].name, "dec_big");
    assert_eq!(r.columns[0].type_name.as_deref(), Some("decimal"));
}

#[tokio::test]
async fn dml_reports_affected_rows_and_ddl_returns_no_result_set() {
    let db = my!();
    let mut s = session(&db).await;
    let (sum, r) = run(&mut s, "CREATE TABLE t (id int PRIMARY KEY, v text)").await;
    assert!(r.columns.is_empty());
    assert_eq!(sum.rows_received, 0);

    let (sum, r) = run(
        &mut s,
        "INSERT INTO t VALUES (1,'x'),(2,'x'),(3,'x'),(4,'x'),(5,'x')",
    )
    .await;
    assert!(r.columns.is_empty(), "INSERT has no result set");
    assert_eq!(sum.affected_rows, Some(5));

    let (sum, _) = run(&mut s, "UPDATE t SET v = 'y' WHERE id <= 2").await;
    assert_eq!(sum.affected_rows, Some(2));
    let (sum, _) = run(&mut s, "DELETE FROM t WHERE id = 99").await;
    assert_eq!(sum.affected_rows, Some(0));
    let (sum, r) = run(&mut s, "SELECT * FROM t WHERE id > 100").await;
    assert_eq!(
        r.columns.len(),
        2,
        "an empty result still reports its columns"
    );
    assert_eq!((sum.rows_received, sum.affected_rows), (0, Some(0)));
}

#[tokio::test]
async fn syntax_error_reports_the_character_position_not_the_byte_position() {
    let db = my!();
    let mut s = session(&db).await;
    let err = s
        .execute(
            &Statement::new("SELECT '日本語' FORM t"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert!(
        err.code.as_deref().is_some_and(|c| c.starts_with("1064")),
        "{err:?}"
    );
    // `FORM` は別名として読まれ、エラーになるのはその次の `t`(文字位置 18)
    assert_eq!(err.position, Some(18), "{err:?}");

    let err = s
        .execute(
            &Statement::new("SELECT * FROM no_such_table"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert!(err.message.contains("no_such_table"), "{err}");
    run(&mut s, "SELECT 1").await;
}

#[tokio::test]
async fn a_statement_with_several_result_sets_reports_the_first_and_keeps_the_session() {
    let db = my!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE PROCEDURE two() BEGIN SELECT 1 AS a; SELECT 2 AS b; END",
    )
    .await;
    let (_, r) = run(&mut s, "CALL two()").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("1"));
    let (_, r) = run(&mut s, "SELECT 'after'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("after"));
}

#[tokio::test]
async fn a_million_rows_stream_through() {
    let db = my!();
    let mut s = session(&db).await;
    let mut sink = Collect::default();
    let started = Instant::now();
    let sum = s
        .execute(
            &Statement::new(format!(
                "SELECT g + 1, g * 2, CONCAT('row ', g) FROM {}",
                numbers(6)
            )),
            &mut sink,
        )
        .await
        .unwrap();
    assert_eq!(sum.rows_received, 1_000_000);
    assert_eq!(sink.count, 1_000_000);
    eprintln!("1M rows in {:?}", started.elapsed());
}
