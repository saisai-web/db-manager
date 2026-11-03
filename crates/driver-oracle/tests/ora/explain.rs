//! 実行計画(`EXPLAIN PLAN` と `V$SQL_PLAN_STATISTICS_ALL`)。

use driver_api::*;
use serde_json::Value;

use crate::common::*;

async fn plan(s: &mut Box<dyn Session>, sql: &str, analyze: bool) -> Vec<Value> {
    let (raw, format) = s
        .explain(sql, analyze)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .expect("Oracle supports plans");
    assert_eq!(format, PlanFormat::OracleJson);
    serde_json::from_str::<Vec<Value>>(&raw).unwrap()
}

fn operations(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["operation"].as_str().unwrap().to_string())
        .collect()
}

/// 計測の設定(`STATISTICS_LEVEL = ALL`)が、このセッションに残っていない。
async fn assert_plan_statistics_off(s: &mut Box<dyn Session>) {
    let status = run(
        s,
        "select session_status from v$statistics_level where statistics_name = 'Plan Execution Statistics'",
    )
    .await
    .1;
    assert_eq!(status.text(0, 0).as_deref(), Some("DISABLED"));
}

async fn setup(s: &mut Box<dyn Session>) {
    exec(
        s,
        "CREATE TABLE big (id NUMBER PRIMARY KEY, v VARCHAR2(10))",
    )
    .await;
    exec(
        s,
        "INSERT INTO big SELECT level, 'v' || level FROM dual CONNECT BY level <= 200",
    )
    .await;
}

#[tokio::test]
async fn an_estimated_plan_lists_the_operations_without_running_the_statement() {
    let db = ora!();
    let mut s = session(&db).await;
    setup(&mut s).await;
    let rows = plan(&mut s, "select * from big where v = 'v7';", false).await;
    assert_eq!(rows[0]["operation"], "SELECT STATEMENT");
    assert!(rows[0]["parent_id"].is_null());
    let scan = rows
        .iter()
        .find(|r| r["operation"] == "TABLE ACCESS")
        .unwrap();
    assert_eq!(scan["options"], "FULL");
    assert_eq!(scan["object_name"], "BIG");
    assert!(
        scan["filter_predicates"].as_str().unwrap().contains("v7"),
        "{scan}"
    );
    assert!(scan["cost"].as_f64().unwrap() > 0.0);
    assert!(scan.get("elapsed_us").is_none(), "no timings without a run");

    // 書き込みの文も、実行はされない(計画だけ)
    plan(&mut s, "delete from big", false).await;
    assert_eq!(
        run(&mut s, "select count(*) from big")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("200")
    );
}

#[tokio::test]
async fn a_measured_plan_has_the_real_rows_and_times() {
    let db = ora!();
    let mut s = session(&db).await;
    setup(&mut s).await;
    let rows = plan(&mut s, "select * from big where id > 100", true).await;
    let ops = operations(&rows);
    assert!(
        ops.iter().any(|o| o.starts_with("TABLE ACCESS"))
            || ops.iter().any(|o| o.contains("INDEX")),
        "{ops:?}"
    );
    let root = &rows[0];
    assert_eq!(root["output_rows"].as_f64(), Some(100.0), "{rows:#?}");
    assert!(root["elapsed_us"].as_f64().is_some());
    assert!(root["starts"].as_f64().unwrap() >= 1.0);
    // 計測の設定は、あとに残らない(計画を求めたあとの通常の文は、計測されない)
    assert_plan_statistics_off(&mut s).await;
}

#[tokio::test]
async fn plans_work_in_read_only_mode_and_inside_a_transaction() {
    let db = ora!();
    let mut s = session(&db).await;
    setup(&mut s).await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await; // 作った直後の表は、読み取り専用で読めない
    s.set_read_only(true).await.unwrap();
    for analyze in [false, true] {
        let rows = plan(&mut s, "select * from big where id = 3", analyze).await;
        assert_eq!(rows[0]["operation"], "SELECT STATEMENT");
    }
    // 計画のあとも、読み取り専用のまま(書き込みは断られる)
    let e = s
        .execute(
            &Statement::new("INSERT INTO big VALUES (999, 'x')"),
            &mut Collect::keeping(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code.as_deref(), Some("ORA-01456"));
    s.set_read_only(false).await.unwrap();

    // 利用者のトランザクションの中でも、そのトランザクションを終わらせない
    exec(&mut s, driver_oracle::OracleDialect.begin_sql()).await;
    exec(&mut s, "INSERT INTO big VALUES (1000, 'y')").await;
    plan(&mut s, "select * from big where id = 1000", true).await;
    assert_eq!(
        run(&mut s, "select count(*) from big")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("201"),
        "the transaction is still open"
    );
    exec(&mut s, "ROLLBACK").await;
    assert_eq!(
        run(&mut s, "select count(*) from big")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("200")
    );
}

#[tokio::test]
async fn a_bad_statement_reports_the_server_error_and_leaves_the_session_usable() {
    let db = ora!();
    let mut s = session(&db).await;
    let e = s
        .explain("select * from no_such_table", false)
        .await
        .unwrap_err();
    assert_eq!(e.code.as_deref(), Some("ORA-00942"), "{e}");
    let e = s
        .explain("select * from no_such_table", true)
        .await
        .unwrap_err();
    assert_eq!(e.code.as_deref(), Some("ORA-00942"), "{e}");
    assert_eq!(
        run(&mut s, "select 1 from dual")
            .await
            .1
            .text(0, 0)
            .as_deref(),
        Some("1")
    );
    assert_plan_statistics_off(&mut s).await;
}
