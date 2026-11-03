//! 実行計画: 構造化された木の取得、実測(ANALYZE)の安全性。

use sqltool_core::*;

use crate::common::*;

async fn explain(h: &mut H, tab: TabId, sql: &str, analyze: bool) -> Event {
    h.core.send(Command::Explain {
        tab,
        sql: sql.into(),
        target: ExecuteTarget::All,
        analyze,
    });
    h.until(|e| {
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

fn walk<'a>(n: &'a PlanNode, out: &mut Vec<&'a PlanNode>) {
    out.push(n);
    n.children.iter().for_each(|c| walk(c, out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_returns_a_plan_tree_without_running_the_statement() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(tab, "create table big (id int primary key, v text); insert into big select g, 'x' from generate_series(1, 2000) g; analyze big").await;

    let ev = explain(&mut h, tab, "select * from big where id = 5", false).await;
    let Event::ExplainReady {
        plan, analyze, sql, ..
    } = ev
    else {
        panic!("{ev:?}")
    };
    assert!(!analyze && sql.contains("big"));
    assert!(plan.root.kind.contains("Scan"), "{:?}", plan.root.kind);
    assert!(
        plan.root
            .target
            .as_deref()
            .is_some_and(|t| t.contains("big"))
    );
    assert!(plan.root.total_cost > 0.0 && plan.execution_ms.is_none());

    // 書き込みの文でも、ANALYZE なしなら安全(実行されない)
    let ev = explain(&mut h, tab, "delete from big", false).await;
    assert!(matches!(ev, Event::ExplainReady { .. }), "{ev:?}");
    let evs = h.exec(tab, "select count(*) from big").await;
    assert_eq!(
        rows(&result_handle(&evs)),
        [[cell("2000")]],
        "EXPLAIN must not execute the DELETE"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn analyze_measures_real_times_but_only_for_read_only_statements() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.exec(
        tab,
        "create table t (a int); insert into t select g from generate_series(1, 1000) g",
    )
    .await;

    let ev = explain(&mut h, tab, "select count(*) from t where a % 2 = 0", true).await;
    let Event::ExplainReady {
        plan,
        analyze: true,
        ..
    } = ev
    else {
        panic!("{ev:?}")
    };
    assert!(plan.execution_ms.is_some() && plan.planning_ms.is_some());
    let mut all = Vec::new();
    walk(&plan.root, &mut all);
    assert!(
        all.iter().all(|n| n.actual_ms.is_some()),
        "every node has measured time"
    );
    assert!(plan.heaviest().self_ms().is_some());

    // 実際に実行してしまう文は、断る。データも変わらない
    let ev = explain(&mut h, tab, "delete from t", true).await;
    let Event::CommandFailed { error, .. } = ev else {
        panic!("{ev:?}")
    };
    assert!(error.message.contains("読み取りだけ"), "{}", error.message);
    let evs = h.exec(tab, "select count(*) from t").await;
    assert_eq!(rows(&result_handle(&evs)), [[cell("1000")]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn databases_without_plan_support_say_so() {
    let mut h = H::start_sqlite().await;
    let dir = tempfile::tempdir().unwrap();
    let tab = h
        .open_sqlite_tab(&dir.path().join("t.db"), EnvLabel::Dev)
        .await;
    let ev = explain(&mut h, tab, "select 1", false).await;
    let Event::CommandFailed { error, .. } = ev else {
        panic!("{ev:?}")
    };
    assert!(error.message.contains("未対応"), "{}", error.message);
}
