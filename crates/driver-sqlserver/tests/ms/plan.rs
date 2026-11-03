//! 実行計画: 推定(文は実行されない)と実測(文を実行する)。どちらも、接続に設定を残さない。

use std::time::Duration;

use driver_api::*;

use crate::common::*;

async fn count(s: &mut Box<dyn Session>, table: &str) -> String {
    let (_, r) = run(s, &format!("SELECT count(*) FROM {table}")).await;
    r.text(0, 0).unwrap()
}

#[tokio::test]
async fn an_estimated_plan_does_not_run_the_statement_and_leaves_the_session_usable() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int primary key, v int)").await;
    let (xml, format) = s
        .explain("INSERT INTO t VALUES (1, 1)", false)
        .await
        .unwrap()
        .expect("SQL Server answers with its own plan");
    assert_eq!(format, PlanFormat::SqlServerXml);
    assert!(xml.contains("<ShowPlanXML"), "{xml}");
    assert!(!xml.contains("RunTimeInformation"), "estimated only");
    assert_eq!(count(&mut s, "t").await, "0", "the INSERT was not executed");
    // SHOWPLAN が残っていると、次の SELECT が結果ではなく計画を返す
    let (_, r) = run(&mut s, "SELECT 41 + 1").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("42"));
}

#[tokio::test]
async fn an_actual_plan_has_runtime_counters_and_the_statement_runs() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int primary key, v int)").await;
    run(&mut s, "INSERT INTO t SELECT TOP 50 ROW_NUMBER() OVER (ORDER BY (SELECT 1)), 1 FROM sys.all_objects").await;
    let (xml, format) = s
        .explain("SELECT * FROM t WHERE v = 1", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(format, PlanFormat::SqlServerXml);
    assert!(xml.contains("ActualRows=\"50\""), "{xml}");
    let (_, r) = run(&mut s, "SELECT 1").await;
    assert_eq!(
        r.text(0, 0).as_deref(),
        Some("1"),
        "STATISTICS XML is off again"
    );
}

#[tokio::test]
async fn a_failing_statement_still_resets_the_setting() {
    let db = ms!();
    let mut s = session(&db).await;
    let err = s
        .explain("SELECT * FROM no_such_table", false)
        .await
        .unwrap_err();
    assert!(err.message.contains("no_such_table"), "{err}");
    let (_, r) = run(&mut s, "SELECT 7").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("7"));
    // 実行時のエラー(実測)でも同じ
    run(&mut s, "CREATE TABLE z (a int)").await;
    run(&mut s, "INSERT INTO z VALUES (0)").await;
    assert!(s.explain("SELECT 1 / a FROM z", true).await.is_err());
    let (_, r) = run(&mut s, "SELECT 8").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("8"));
}

#[tokio::test]
async fn a_statement_without_a_plan_is_reported() {
    let db = ms!();
    let mut s = session(&db).await;
    let err = s.explain("SET NOCOUNT ON", false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    let (_, r) = run(&mut s, "SELECT 1").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("1"));
}

#[tokio::test]
async fn cancel_stops_an_actual_plan_and_the_session_recovers() {
    let db = ms!();
    let mut s = session(&db).await;
    let canceller = s.canceller();
    let task = tokio::spawn(async move {
        let r = s.explain("WAITFOR DELAY '00:00:30'", true).await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    canceller.cancel().await.unwrap();
    let (mut s, r) = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("cancel must be quick")
        .unwrap();
    assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
    let (_, r) = run(&mut s, "SELECT 9").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("9"));
}
