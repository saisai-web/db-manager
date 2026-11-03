//! 実行と結果、エラー位置、安全確認(読み取り専用、確認、トランザクション)。

use sqltool_core::*;

use super::My;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_statements_and_see_results_just_like_with_postgresql() {
    let mut m = My::start(my!(), EnvLabel::Dev).await;
    let evs = m.h.exec(m.tab, "create table t (id int primary key, name text); insert into t values (1, 'a'), (2, null), (3, ''); select * from t order by id").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
    let r = result_handle(&evs);
    assert_eq!(
        rows(&r),
        [
            [cell("1"), cell("a")],
            [cell("2"), None],
            [cell("3"), cell("")]
        ]
    );
    assert_eq!(r.columns()[0].class, ValueClass::Number);

    // バックスラッシュと日本語も、そのまま往復する
    let got = m.rows(r"select 'a\\b', '日本語'").await;
    assert_eq!(got[0], [cell(r"a\b"), cell("日本語")]);

    // エラー位置も、エディタ上の位置に直る(2 文目の中の位置)
    let sql = "select 1;\nselect '日本語', from t";
    let evs = m.h.exec(m.tab, sql).await;
    let StatementOutcome::Failed { editor_offset, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    let off = editor_offset.expect("offset");
    assert_eq!(&sql[off..off + 4], "from");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safety_rules_work_with_mysqls_dialect() {
    let mut m = My::start(my!(), EnvLabel::Production).await;
    // 本番ラベルは読み取り専用で始まり、書き込みは、1 文も実行されない
    let evs = m.h.exec(m.tab, "select 1; create table t (a int)").await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    m.set_read_only(false).await;
    m.h.exec(
        m.tab,
        "create table t (a int); insert into t values (1), (2)",
    )
    .await;

    // DB 側の読み取り専用(SET SESSION TRANSACTION READ ONLY)でも止まる
    m.set_read_only(true).await;
    m.set_read_only(false).await;
    assert_eq!(m.rows("select count(*) from t").await[0][0], cell("2"));

    // WHERE のない DELETE は、確認が出る。断ると、実行されない
    m.h.send_exec(
        m.tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs =
        m.h.until(|e| matches!(e, Event::ConfirmationRequired { .. }))
            .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items[0].kind, ConfirmKind::UnfilteredDelete);
    m.h.core.send(Command::Confirm { id, accept: false });
    m.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(m.rows("select count(*) from t").await[0][0], cell("2"));

    // トランザクションの状態も追跡できる(START TRANSACTION と BEGIN のどちらでも)
    for begin in ["start transaction", "begin"] {
        let evs = m.h.exec(m.tab, begin).await;
        assert!(
            evs.iter().any(|e| matches!(
                e,
                Event::TransactionChanged {
                    state: TxnState::Open,
                    ..
                }
            )),
            "{begin}: {evs:?}"
        );
        m.h.exec(m.tab, "rollback").await;
        assert!(
            m.h.core.pending_transactions().is_empty(),
            "{begin}: {:?}",
            m.h.core.pending_transactions()
        );
    }
}
