//! 読み取り専用モード(文ごとの `SET TRANSACTION READ ONLY` と ROLLBACK)。

use driver_api::*;
use driver_oracle::OracleDialect;

use crate::common::*;

async fn fails_with(s: &mut Box<dyn Session>, sql: &str) -> DriverError {
    match s
        .execute(&Statement::new(sql), &mut Collect::keeping())
        .await
    {
        Ok(_) => panic!("{sql} should be refused"),
        Err(e) => e,
    }
}

async fn count(s: &mut Box<dyn Session>, table: &str) -> String {
    run(s, &format!("SELECT count(*) FROM {table}"))
        .await
        .1
        .text(0, 0)
        .unwrap()
}

#[tokio::test]
async fn read_only_mode_lets_reads_through_and_stops_writes() {
    let db = ora!();
    let mut s = session(&db).await;
    exec(&mut s, "CREATE TABLE t (id NUMBER)").await;
    exec(&mut s, "INSERT INTO t VALUES (1)").await;
    s.set_read_only(true).await.unwrap();

    // 作った直後の表も読める(読み取り専用のトランザクションが読めるまで、やり直す)
    assert_eq!(count(&mut s, "t").await, "1");
    assert_eq!(
        run(&mut s, "SELECT 1 FROM dual")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("1")
    );

    // 書き込みは、サーバーが拒否する
    for sql in [
        "INSERT INTO t VALUES (2)",
        "UPDATE t SET id = 5",
        "DELETE FROM t",
    ] {
        let e = fails_with(&mut s, sql).await;
        assert_eq!(e.code.as_deref(), Some("ORA-01456"), "{sql}: {e}");
    }
    // DDL と PL/SQL は、読み取り専用のトランザクションを暗黙にコミットして抜けられるので、断る
    for sql in [
        "CREATE TABLE u (a NUMBER)",
        "DROP TABLE t",
        "TRUNCATE TABLE t",
        "COMMENT ON TABLE t IS 'x'",
        "GRANT SELECT ON t TO PUBLIC",
        "BEGIN NULL; END;",
        "BEGIN EXECUTE IMMEDIATE 'DROP TABLE t'; END;",
    ] {
        let e = fails_with(&mut s, sql).await;
        assert!(e.message.contains("読み取り専用"), "{sql}: {e}");
    }
    // 行のロックも取れない
    let e = fails_with(&mut s, "SELECT * FROM t FOR UPDATE").await;
    assert!(e.message.contains("ORA-"), "{e}");
    // セッションの設定は変えられる
    exec(&mut s, "ALTER SESSION SET NLS_LANGUAGE = 'AMERICAN'").await;

    // どれも効いていない。戻せば、また書ける
    s.set_read_only(false).await.unwrap();
    assert_eq!(count(&mut s, "t").await, "1");
    exec(&mut s, "INSERT INTO t VALUES (2)").await;
    assert_eq!(count(&mut s, "t").await, "2");
}

#[tokio::test]
async fn read_only_transactions_stay_read_only_until_they_end() {
    let db = ora!();
    let mut s = session(&db).await;
    exec(&mut s, "CREATE TABLE t (id NUMBER)").await;
    // 作った直後の表は、読み取り専用のトランザクションが始まっていると読めない(数秒のあいだ)
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    s.set_read_only(true).await.unwrap();

    // 利用者のトランザクションも、読み取り専用で始まる
    exec(&mut s, OracleDialect.begin_sql()).await;
    assert_eq!(count(&mut s, "t").await, "0");
    let e = fails_with(&mut s, "INSERT INTO t VALUES (1)").await;
    assert_eq!(e.code.as_deref(), Some("ORA-01456"));
    // 読み取り専用を外す SET TRANSACTION は通さない
    let e = fails_with(&mut s, "SET TRANSACTION READ WRITE").await;
    assert!(e.message.contains("読み取り専用"), "{e}");
    exec(&mut s, "COMMIT").await;
    // 終わったあとは、また、文ごとに読み取り専用
    let e = fails_with(&mut s, "INSERT INTO t VALUES (1)").await;
    assert_eq!(e.code.as_deref(), Some("ORA-01456"));
    assert_eq!(count(&mut s, "t").await, "0");
}
