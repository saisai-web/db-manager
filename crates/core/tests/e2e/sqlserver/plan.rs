//! SQL Server の実行計画(推定と実測)と、読み取り専用モード(文ごとのトランザクションでの取り消し)。

use sqltool_core::*;

use super::Ms;
use crate::common::*;

async fn explain(m: &mut Ms, sql: &str, analyze: bool) -> Event {
    m.h.core.send(Command::Explain {
        tab: m.tab,
        sql: sql.into(),
        target: ExecuteTarget::All,
        analyze,
    });
    m.h.until(|e| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_returns_a_plan_tree_with_estimates_and_actual_timings() {
    let mut m = Ms::start(ms!(), EnvLabel::Dev).await;
    m.h.exec(
        m.tab,
        "create table a (id int primary key, v nvarchar(20)); create table b (id int primary key, aid int, w int);
         insert into a select top 200 row_number() over (order by (select 1)), 'x' from sys.all_objects;
         insert into b select id, id, id from a",
    )
    .await;
    let join = "select a.v, count(*) from a join b on b.aid = a.id where b.w > 3 group by a.v";

    // 推定: 実行せず、演算子の木と見積もりが返る
    let Event::ExplainReady { plan, analyze, .. } = explain(&mut m, join, false).await else {
        panic!("explain failed")
    };
    assert!(!analyze);
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    assert!(all.len() >= 4, "{all:#?}");
    assert!(
        all.iter().any(|n| n.kind.contains("Join")
            || n.kind.contains("Nested Loops")
            || n.kind.contains("Hash Match")),
        "{all:#?}"
    );
    let targets: Vec<_> = all.iter().filter_map(|n| n.target.as_deref()).collect();
    assert!(
        targets.iter().any(|t| t.starts_with("a using "))
            && targets.iter().any(|t| t.starts_with("b using ")),
        "{targets:?}"
    );
    assert!(plan.root.total_cost > 0.0 && plan.root.rows >= 1.0);
    assert_eq!(plan.execution_ms, None, "no timings without analyze");
    assert!(
        all.iter()
            .any(|n| n.conditions.iter().any(|(_, c)| c.contains("[w]"))),
        "the filter on b.w is shown: {all:#?}"
    );

    // 実測: 文を実行し、各演算子に行数が付く
    let Event::ExplainReady { plan, .. } = explain(&mut m, join, true).await else {
        panic!("analyze failed")
    };
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    assert!(
        plan.execution_ms.is_some() && all.iter().all(|n| n.actual_rows.is_some()),
        "{plan:#?}"
    );
    let scan_rows: Vec<f64> = all
        .iter()
        .filter(|n| n.kind.contains("Scan") || n.kind.contains("Seek"))
        .filter_map(|n| n.actual_rows)
        .collect();
    assert!(
        scan_rows.contains(&200.0),
        "a is read in full: {scan_rows:?}"
    );

    // 書き込みを含む文には、実測を使わせない。推定は、文を実行しない
    let ev = explain(&mut m, "delete from a where id = 1", true).await;
    assert!(matches!(ev, Event::CommandFailed { .. }), "{ev:?}");
    let ev = explain(&mut m, "delete from a where id = 1", false).await;
    assert!(matches!(ev, Event::ExplainReady { .. }), "{ev:?}");
    assert_eq!(m.rows("select count(*) from a").await[0][0], cell("200"));
    // 計画のあとも、ふつうに実行できる
    assert_eq!(m.rows("select 1").await[0][0], cell("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_database_side_net_undoes_writes_that_the_statement_check_lets_through() {
    let mut m = Ms::start(ms!(), EnvLabel::Production).await;
    // 本番ラベルは読み取り専用で始まる。`SELECT ... INTO` は、字句の判定では SELECT なので、
    // core は通す。SQL Server 側の備えが、作られたテーブルを取り消す
    m.h.exec(m.tab, "select 1 as a into made").await;
    m.h.core.send(Command::SetReadOnly {
        tab: m.tab,
        read_only: false,
    });
    m.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        m.rows("select count(*) from sys.tables where name = 'made'")
            .await[0][0],
        cell("0")
    );
    // 書き込み可能に切り替えたあとは、同じ文が残る
    m.h.exec(m.tab, "select 1 as a into made").await;
    assert_eq!(
        m.rows("select count(*) from sys.tables where name = 'made'")
            .await[0][0],
        cell("1")
    );
}
