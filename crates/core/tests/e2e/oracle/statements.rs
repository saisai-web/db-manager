//! 実行と結果、エラー位置、安全確認(読み取り専用、確認)、1 文ずつの確定。

use sqltool_core::*;

use super::Ora;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_statements_and_see_results_just_like_with_postgresql() {
    let mut o = Ora::start(ora!(), EnvLabel::Dev).await;
    // Oracle では、空文字列は NULL になる。文末の `;` は、文の区切りで、DB には送られない
    let evs = o.h.exec(o.tab, "create table t (id number primary key, name varchar2(20)); insert into t values (1, 'a'); insert into t values (2, null); insert into t values (3, ''); select * from t order by id").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
    let r = result_handle(&evs);
    assert_eq!(
        rows(&r),
        [[cell("1"), cell("a")], [cell("2"), None], [cell("3"), None]]
    );
    assert_eq!(r.columns()[0].class, ValueClass::Number);

    // バックスラッシュは通常の文字。日本語も、そのまま往復する
    let got = o.rows(r"select 'a\b', '日本語' from dual").await;
    assert_eq!(got[0], [cell(r"a\b"), cell("日本語")]);

    // エラー位置も、エディタ上の位置に直る(2 文目の中の位置)
    let sql = "select 1 from dual;\nselect '日本語' x, 1 from t y where";
    let evs = o.h.exec(o.tab, sql).await;
    let StatementOutcome::Failed { editor_offset, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    assert!(editor_offset.is_some(), "{evs:?}");
    let sql = "select 1 from dual;\nselect '日本語', from t";
    let evs = o.h.exec(o.tab, sql).await;
    let StatementOutcome::Failed { editor_offset, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    let off = editor_offset.expect("offset");
    assert_eq!(&sql[off..off + 4], "from");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safety_rules_work_with_oracles_dialect() {
    let mut o = Ora::start(ora!(), EnvLabel::Production).await;
    // 本番ラベルは読み取り専用で始まり、書き込みは、1 文も実行されない
    // (DB 側の読み取り専用トランザクションとは別に、core の判定が先に働く)
    let evs =
        o.h.exec(o.tab, "select 1 from dual; create table t (a int)")
            .await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    o.h.core.send(Command::SetReadOnly {
        tab: o.tab,
        read_only: false,
    });
    o.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    o.h.exec(
        o.tab,
        "create table t (a int); insert into t values (1); insert into t values (2)",
    )
    .await;
    assert_eq!(o.rows("select count(*) from t").await[0][0], cell("2"));

    // WHERE のない DELETE は、確認が出る。断ると、実行されない
    o.h.send_exec(
        o.tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs =
        o.h.until(|e| matches!(e, Event::ConfirmationRequired { .. }))
            .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items[0].kind, ConfirmKind::UnfilteredDelete);
    o.h.core.send(Command::Confirm { id, accept: false });
    o.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(o.rows("select count(*) from t").await[0][0], cell("2"));
}

/// 他の DB と同じく、1 文ずつ確定される(Oracle の既定の「COMMIT まで未確定」だと、UI が未コミットを
/// 検知できず、タブを閉じると変更が消え、ロックも残る)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_statement_is_committed_so_other_sessions_see_it() {
    let mut a = Ora::start(ora!(), EnvLabel::Dev).await;
    a.h.exec(a.tab, "create table t (a number)").await;
    a.h.exec(a.tab, "insert into t values (1)").await;
    // 同じ接続設定で、別のタブ(別のセッション)を開く
    let conn = a.h.core_connection_id(a.tab);
    let b_tab = a.h.open_tab_for(&conn).await;
    let evs = a.h.exec(b_tab, "select count(*) from t").await;
    assert_eq!(rows(&result_handle(&evs))[0][0], cell("1"));
    assert!(a.h.core.pending_transactions().is_empty());
}
