//! 値の正確さ、DML の影響行数、エラー、大量行の受信。

use std::time::Instant;

use driver_api::*;

use crate::common::*;

#[tokio::test]
async fn connects_and_reports_server_version() {
    let db = ms!();
    let s = session(&db).await;
    assert!(
        s.server_info().version.starts_with("Microsoft SQL Server"),
        "{}",
        s.server_info().version
    );
    s.close().await.unwrap();
}

#[tokio::test]
async fn values_come_back_exactly_as_the_server_formats_them() {
    let db = ms!();
    let mut s = session(&db).await;
    let (sum, r) = run(
        &mut s,
        "SELECT CAST('12345678901234567890.123456789012345678' AS DECIMAL(38,18)) AS dec_big,
                CAST(9223372036854775807 AS BIGINT) AS maxint,
                CAST(0.1 AS FLOAT) + CAST(0.2 AS FLOAT) AS f,
                CAST('2026-11-02 12:34:56.1234567' AS DATETIME2(7)) AS ts,
                CAST('2026-11-02 12:34:56.123456 +09:00' AS DATETIMEOFFSET(6)) AS tso,
                N'日本語のテキスト' AS jp,
                CAST(NULL AS NVARCHAR(10)) AS nul,
                N'' AS emp,
                CAST(1 AS BIT) AS b,
                0x00ff10 AS bin,
                N'a\"b' AS quote,
                CAST(1.5 AS DECIMAL(5,2)) AS trl,
                CAST('10:20:30.5' AS TIME(3)) AS t,
                CAST('2026-11-02' AS DATE) AS d,
                CAST('2026-11-02 12:34:56.123' AS DATETIME) AS dt,
                CAST('b2d4f1a0-0000-4000-8000-0123456789ab' AS UNIQUEIDENTIFIER) AS g,
                CAST(-0.5 AS DECIMAL(3,1)) AS neg,
                CAST(12.3456 AS MONEY) AS m",
    )
    .await;
    assert_eq!(sum.rows_received, 1);
    assert_eq!(sum.affected_rows, Some(1));
    assert!(!sum.stopped_early);

    let t = |i| r.text(0, i);
    assert_eq!(
        t(0).as_deref(),
        Some("12345678901234567890.123456789012345678")
    );
    assert_eq!(t(1).as_deref(), Some("9223372036854775807"));
    assert_eq!(t(2).as_deref(), Some("0.30000000000000004"));
    assert_eq!(t(3).as_deref(), Some("2026-11-02 12:34:56.1234567"));
    assert_eq!(t(4).as_deref(), Some("2026-11-02 12:34:56.123456 +09:00"));
    assert_eq!(t(5).as_deref(), Some("日本語のテキスト"));
    assert_eq!(r.rows[0][6], None, "NULL");
    assert_eq!(
        r.rows[0][7].as_deref(),
        Some(&b""[..]),
        "empty string is not NULL"
    );
    assert_eq!(t(8).as_deref(), Some("1"));
    assert_eq!(r.rows[0][9].as_deref(), Some(&[0x00, 0xff, 0x10][..]));
    assert_eq!(t(10).as_deref(), Some("a\"b"));
    assert_eq!(t(11).as_deref(), Some("1.50"), "scale is preserved");
    assert_eq!(t(12).as_deref(), Some("10:20:30.500"));
    assert_eq!(t(13).as_deref(), Some("2026-11-02"));
    assert_eq!(t(14).as_deref(), Some("2026-11-02 12:34:56.123"));
    assert_eq!(
        t(15).as_deref(),
        Some("B2D4F1A0-0000-4000-8000-0123456789AB")
    );
    assert_eq!(t(16).as_deref(), Some("-0.5"));
    assert_eq!(t(17).as_deref(), Some("12.3456"));

    let classes: Vec<_> = r.columns.iter().map(|c| c.class).collect();
    assert_eq!(
        classes,
        [
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Temporal,
            ValueClass::Temporal,
            ValueClass::Text,
            ValueClass::Text,
            ValueClass::Text,
            ValueClass::Bool,
            ValueClass::Binary,
            ValueClass::Text,
            ValueClass::Number,
            ValueClass::Temporal,
            ValueClass::Temporal,
            ValueClass::Temporal,
            ValueClass::Text,
            ValueClass::Number,
            ValueClass::Number,
        ]
    );
    assert_eq!(r.columns[0].name, "dec_big");
}

#[tokio::test]
async fn text_in_other_code_pages_and_xml_round_trip() {
    let db = ms!();
    let mut s = session(&db).await;
    let (_, r) = run(
        &mut s,
        "SELECT CAST(N'日本語' COLLATE Japanese_CI_AS AS VARCHAR(20)) AS sjis, CAST(N'<a b=\"1\">x</a>' AS XML) AS x",
    )
    .await;
    assert_eq!(r.text(0, 0).as_deref(), Some("日本語"));
    assert_eq!(r.text(0, 1).as_deref(), Some("<a b=\"1\">x</a>"));
}

#[tokio::test]
async fn dml_reports_affected_rows_and_ddl_returns_no_result_set() {
    let db = ms!();
    let mut s = session(&db).await;
    let (sum, r) = run(
        &mut s,
        "CREATE TABLE t (id int PRIMARY KEY, v nvarchar(max))",
    )
    .await;
    assert!(r.columns.is_empty());
    assert_eq!(sum.rows_received, 0);
    assert_eq!(sum.affected_rows, Some(0));

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
    let db = ms!();
    let mut s = session(&db).await;
    let err = s
        .execute(
            &Statement::new("SELECT N'日本語' FRM tbl"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert_eq!(err.code.as_deref(), Some("102"), "{err:?}");
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
async fn an_error_in_the_middle_of_a_result_is_reported_and_the_session_survives() {
    let db = ms!();
    let mut s = session(&db).await;
    let err = s
        .execute(
            &Statement::new("SELECT 1 / (g - 3) FROM (VALUES (1),(2),(3),(4)) v(g)"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert_eq!(err.code.as_deref(), Some("8134"), "{err:?}");
    let (_, r) = run(&mut s, "SELECT 'alive'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("alive"));
}

#[tokio::test]
async fn a_batch_with_several_result_sets_reports_the_first_and_keeps_the_session() {
    let db = ms!();
    let mut s = session(&db).await;
    let (sum, r) = run(&mut s, "SELECT 1 AS a; SELECT 2 AS b, 3 AS c").await;
    assert_eq!(r.columns.len(), 1);
    assert_eq!(r.text(0, 0).as_deref(), Some("1"));
    assert_eq!(sum.rows_received, 1);
    let (_, r) = run(&mut s, "SELECT 'after'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("after"));
}

#[tokio::test]
async fn a_million_rows_stream_through() {
    let db = ms!();
    let mut s = session(&db).await;
    let mut sink = Collect::default();
    let started = Instant::now();
    let sum = s
        .execute(
            &Statement::new(format!(
                "SELECT g + 1, g * 2, CONCAT(N'row ', g) FROM {}",
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
