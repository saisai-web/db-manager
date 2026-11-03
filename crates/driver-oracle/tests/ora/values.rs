//! 値の正確さ、DML の影響行数、エラー、大量行の受信。

use std::time::Instant;

use driver_api::*;

use crate::common::*;

#[tokio::test]
async fn connects_and_reports_server_version() {
    let db = ora!();
    let s = session(&db).await;
    assert!(
        s.server_info().version.starts_with("Oracle Database 2"),
        "{}",
        s.server_info().version
    );
    s.close().await.unwrap();
}

#[tokio::test]
async fn values_come_back_exactly_as_the_server_has_them() {
    let db = ora!();
    let mut s = session(&db).await;
    let (sum, r) = run(
        &mut s,
        "SELECT CAST('12345678901234567890123456789012345678' AS NUMBER(38)) AS big,
                12345678901234567890.123456789012345678 AS dec_big,
                1.50 AS trl,
                -0.000123 AS small,
                CAST(0.1 AS BINARY_DOUBLE) AS bd,
                CAST(1.5 AS BINARY_FLOAT) AS bf,
                CAST('NaN' AS BINARY_DOUBLE) AS nan,
                TO_DATE('2026-11-02 12:34:56', 'YYYY-MM-DD HH24:MI:SS') AS d,
                CAST(TIMESTAMP '2026-11-02 12:34:56.123456' AS TIMESTAMP(6)) AS ts,
                CAST(TIMESTAMP '2026-11-02 12:34:56.123456789' AS TIMESTAMP(9)) AS ts9,
                CAST(TIMESTAMP '2026-11-02 12:34:56.5 +09:00' AS TIMESTAMP(6) WITH TIME ZONE) AS tstz,
                INTERVAL '1-2' YEAR TO MONTH AS ym,
                INTERVAL '1 2:3:4.5' DAY TO SECOND AS ds,
                '日本語のテキスト' AS jp,
                TO_NCHAR('日本語nchar') AS nch,
                CAST('abc' AS CHAR(5)) AS fixed,
                HEXTORAW('00FF10') AS bin,
                NULL AS nul,
                '' AS emp,
                TO_CLOB('clob text') AS c,
                TRUE AS flag
         FROM dual",
    )
    .await;
    assert_eq!(sum.rows_received, 1);
    assert_eq!(sum.affected_rows, Some(1));

    let t = |c| r.text(0, c);
    assert_eq!(
        t(0).as_deref(),
        Some("12345678901234567890123456789012345678")
    );
    assert_eq!(
        t(1).as_deref(),
        Some("12345678901234567890.123456789012345678")
    );
    assert_eq!(t(2).as_deref(), Some("1.5"), "NUMBER has no trailing zeros");
    assert_eq!(t(3).as_deref(), Some("-0.000123"), "{:?}", t(3));
    assert_eq!(t(4).as_deref(), Some("0.1"));
    assert_eq!(t(5).as_deref(), Some("1.5"));
    assert_eq!(t(6).as_deref(), Some("Nan"));
    assert_eq!(t(7).as_deref(), Some("2026-11-02 12:34:56"));
    assert_eq!(t(8).as_deref(), Some("2026-11-02 12:34:56.123456"));
    assert_eq!(t(9).as_deref(), Some("2026-11-02 12:34:56.123456789"));
    assert_eq!(
        t(10).as_deref(),
        Some("2026-11-02 03:34:56.500000 UTC"),
        "the library returns the instant in UTC"
    );
    assert_eq!(t(11).as_deref(), Some("+01-02"));
    assert_eq!(t(12).as_deref(), Some("+01 02:03:04.500000000"));
    assert_eq!(t(13).as_deref(), Some("日本語のテキスト"));
    assert_eq!(t(14).as_deref(), Some("日本語nchar"));
    assert_eq!(t(15).as_deref(), Some("abc  "), "CHAR is blank-padded");
    assert_eq!(r.rows[0][16].as_deref(), Some(&[0x00, 0xff, 0x10][..]));
    assert_eq!(r.rows[0][17], None, "NULL");
    assert_eq!(r.rows[0][18], None, "Oracle treats '' as NULL");
    assert_eq!(t(19).as_deref(), Some("clob text"));
    assert_eq!(t(20).as_deref(), Some("TRUE"));

    let classes: Vec<_> = r.columns.iter().map(|c| c.class).collect();
    use ValueClass::*;
    assert_eq!(
        classes,
        [
            Number, Number, Number, Number, Number, Number, Number, Temporal, Temporal, Temporal,
            Temporal, Temporal, Temporal, Text, Text, Text, Binary, Text, Text, Text, Bool
        ]
    );
    assert_eq!(r.columns[0].name, "BIG");
    assert_eq!(r.columns[8].type_name.as_deref(), Some("TIMESTAMP(6)"));
}

#[tokio::test]
async fn rows_made_only_of_nulls_are_not_lost() {
    let db = ora!();
    let mut s = session(&db).await;
    for (sql, rows) in [
        ("SELECT NULL FROM dual", 1),
        ("SELECT '' FROM dual", 1),
        ("SELECT NULL, NULL FROM dual", 1),
        ("SELECT NULL AS a FROM dual CONNECT BY level <= 3", 3),
        ("SELECT 1 FROM dual WHERE 1 = 0", 0),
    ] {
        let (sum, r) = run(&mut s, sql).await;
        assert_eq!(r.rows.len(), rows, "{sql}");
        assert_eq!(sum.rows_received, rows as u64, "{sql}");
        assert!(
            !r.columns.is_empty(),
            "{sql}: columns are reported even without rows"
        );
    }
}

#[tokio::test]
async fn dml_reports_affected_rows_and_ddl_returns_no_result_set() {
    let db = ora!();
    let mut s = session(&db).await;
    let (sum, r) = run(
        &mut s,
        "CREATE TABLE t (id NUMBER PRIMARY KEY, v VARCHAR2(20))",
    )
    .await;
    assert!(r.columns.is_empty());
    assert_eq!(sum.rows_received, 0);

    let (sum, r) = run(
        &mut s,
        "INSERT INTO t SELECT level, 'x' FROM dual CONNECT BY level <= 5",
    )
    .await;
    assert!(r.columns.is_empty(), "INSERT has no result set");
    assert_eq!(sum.affected_rows, Some(5));

    assert_eq!(
        exec(&mut s, "UPDATE t SET v = 'y' WHERE id <= 2")
            .await
            .affected_rows,
        Some(2)
    );
    assert_eq!(
        exec(&mut s, "DELETE FROM t WHERE id = 99")
            .await
            .affected_rows,
        Some(0)
    );
    // 文末の `;` は、Oracle では不要。付いていても動く
    assert_eq!(
        exec(&mut s, "DELETE FROM t WHERE id = 5;")
            .await
            .affected_rows,
        Some(1)
    );
    let (sum, r) = run(&mut s, "SELECT * FROM t WHERE id > 100").await;
    assert_eq!(
        r.columns.len(),
        2,
        "an empty result still reports its columns"
    );
    assert_eq!((sum.rows_received, sum.affected_rows), (0, Some(0)));

    // PL/SQL のブロックは、末尾の `;` も文の一部
    let sum = exec(&mut s, "BEGIN UPDATE t SET v = 'z'; END;").await;
    assert_eq!(sum.affected_rows, Some(0));
    let (_, r) = run(&mut s, "SELECT count(*) FROM t WHERE v = 'z'").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("4"));
}

#[tokio::test]
async fn syntax_error_reports_the_character_position_not_the_byte_position() {
    let db = ora!();
    let mut s = session(&db).await;
    let sql = "SELECT '日本語' AS x, 1 FORM dual";
    let err = s
        .execute(&Statement::new(sql), &mut Collect::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    assert_eq!(err.code.as_deref(), Some("ORA-00923"), "{err:?}");
    let pos = err.position.expect("a parse error has a position");
    assert_eq!(
        &sql.chars().skip(pos).take(4).collect::<String>(),
        "dual",
        "{err:?}"
    );

    let err = s
        .execute(
            &Statement::new("SELECT * FROM no_such_table"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code.as_deref(), Some("ORA-00942"));
    assert!(err.message.contains("NO_SUCH_TABLE"), "{err}");
    // 実行時のエラーは、文のどこでもないので、位置を付けない
    let err = s
        .execute(
            &Statement::new("SELECT 1/0 FROM dual"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (err.code.as_deref(), err.position),
        (Some("ORA-01476"), None)
    );
    run(&mut s, "SELECT 1 FROM dual").await;
}

#[tokio::test]
async fn a_million_rows_stream_through_without_loss() {
    let db = ora!();
    let mut s = session(&db).await;
    for n in [1_000u64, 100_000, 1_000_000] {
        let mut sink = Collect::default();
        let started = Instant::now();
        let sum = s
            .execute(
                &Statement::new(format!(
                    "SELECT level, 'row ' || level FROM dual CONNECT BY level <= {n}"
                )),
                &mut sink,
            )
            .await
            .unwrap();
        assert_eq!(sum.rows_received, n);
        assert_eq!(sink.count as u64, n);
        assert_eq!(sum.affected_rows, Some(n));
        eprintln!("{n} rows in {:?}", started.elapsed());
    }
    // 中身も正しい(順序と値)
    let mut sink = Collect::keeping();
    s.execute(
        &Statement::new("SELECT level, 'row ' || level FROM dual CONNECT BY level <= 5000"),
        &mut sink,
    )
    .await
    .unwrap();
    for (i, row) in sink.rows.iter().enumerate() {
        let n = i + 1;
        assert_eq!(sink.text(i, 0).as_deref(), Some(n.to_string().as_str()));
        assert_eq!(
            sink.text(i, 1).as_deref(),
            Some(format!("row {n}").as_str()),
            "{row:?}"
        );
    }
}
