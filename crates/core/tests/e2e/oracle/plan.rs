//! 実行計画と、読み取り専用モード(DB 側の、文ごとの読み取り専用トランザクション)。

use sqltool_core::*;

use super::Ora;
use crate::common::*;

async fn explain(o: &mut Ora, sql: &str, analyze: bool) -> Event {
    o.h.core.send(Command::Explain {
        tab: o.tab,
        sql: sql.into(),
        target: ExecuteTarget::All,
        analyze,
    });
    o.h.until(|e| {
        matches!(
            e,
            Event::ExplainReady { .. }
                | Event::CommandFailed {
                    command: "Explain",
                    ..
                }
        )
    })
    .await
    .pop()
    .unwrap()
}

async fn set_read_only(o: &mut Ora, read_only: bool) {
    o.h.core.send(Command::SetReadOnly {
        tab: o.tab,
        read_only,
    });
    o.h.until(|e| matches!(e, Event::ReadOnlyChanged { read_only: r, .. } if *r == read_only))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_returns_a_plan_tree_on_oracle() {
    let mut o = Ora::start(ora!(), EnvLabel::Dev).await;
    o.h.exec(
        o.tab,
        "create table big (id number primary key, v varchar2(10)); \
         insert into big select level, 'v' || level from dual connect by level <= 200",
    )
    .await;

    // 実行せずに、見積もりの木を求める
    let Event::ExplainReady { plan, analyze, .. } =
        explain(&mut o, "select * from big where v = 'v7'", false).await
    else {
        panic!("explain failed")
    };
    assert!(!analyze);
    assert_eq!(plan.root.kind, "SELECT STATEMENT");
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    let scan = all
        .iter()
        .find(|n| n.kind == "TABLE ACCESS FULL")
        .unwrap_or_else(|| panic!("{all:#?}"));
    assert_eq!(scan.target.as_deref(), Some("BIG"));
    assert!(
        scan.conditions
            .iter()
            .any(|(k, v)| k == "Filter" && v.contains("v7"))
    );
    assert_eq!(plan.execution_ms, None, "no timings without analyze");

    // 実測つき: 実際に実行して、時間と行数が付く
    let Event::ExplainReady { plan, .. } =
        explain(&mut o, "select * from big where id > 100", true).await
    else {
        panic!("analyze failed")
    };
    assert!(plan.execution_ms.is_some(), "{plan:#?}");
    assert_eq!(plan.root.actual_rows, Some(100.0), "{plan:#?}");

    // 書き込みを含む文には、実測を使わせない。実行もされない
    let ev = explain(&mut o, "delete from big where id = 1", true).await;
    assert!(matches!(ev, Event::CommandFailed { .. }), "{ev:?}");
    assert_eq!(o.rows("select count(*) from big").await[0][0], cell("200"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_mode_is_enforced_by_the_database_as_well() {
    let mut o = Ora::start(ora!(), EnvLabel::Dev).await;
    o.h.exec(o.tab, "create table t (a number); insert into t values (1)")
        .await;
    set_read_only(&mut o, true).await;

    // 作った直後の表も読める。計画も求められる
    assert_eq!(o.rows("select count(*) from t").await[0][0], cell("1"));
    assert!(matches!(
        explain(&mut o, "select * from t", true).await,
        Event::ExplainReady { .. }
    ));
    // 書き込みは、core が実行する前に断る
    let evs = o.h.exec(o.tab, "insert into t values (2)").await;
    assert!(
        matches!(status(&evs), ExecutionStatus::RefusedReadOnly { .. }),
        "{evs:?}"
    );

    // 外せば書ける。戻すと、また、読むだけになる
    set_read_only(&mut o, false).await;
    o.h.exec(o.tab, "insert into t values (2)").await;
    set_read_only(&mut o, true).await;
    assert_eq!(o.rows("select count(*) from t").await[0][0], cell("2"));
    set_read_only(&mut o, false).await;
    assert!(o.h.core.pending_transactions().is_empty());
}
