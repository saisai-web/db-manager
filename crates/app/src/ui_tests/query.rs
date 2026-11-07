//! 右クリックのメニューが作る SQL の雛形が、Query入力の新しいタブに入ることと、
//! Query タブの追加・切り替え・削除、「100 行を表示」が行数を絞ることを確かめる。

use gpui_kit::TestAppContext;
use sqltool_core::{SqlKind, TableRef};

use super::harness::*;

fn docs(cx: &mut TestAppContext, f: &Fixture) -> Vec<(String, String)> {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .queries
            .iter()
            .map(|q| (q.title.to_string(), q.editor.read(cx).value().to_string()))
            .collect()
    })
}

#[gpui_kit::test]
fn a_generated_update_opens_in_a_new_query_tab_and_leaves_the_draft(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "create table gen_t (id int primary key, name text)");
    cx.update(|cx| {
        f.model.update(cx, |m, cx| {
            let tab = m.active.unwrap();
            m.tab(tab).unwrap().editor().update(cx, |_, _| {});
            m.request_sql(tab, TableRef::new("public", "gen_t"), SqlKind::Update)
        })
    });
    wait_until(cx, "the template in a new tab", |cx| {
        docs(cx, &f).len() == 2
    });
    let docs = docs(cx, &f);
    assert!(
        docs[0].1.starts_with("create table gen_t"),
        "the draft stays"
    );
    assert_eq!(docs[1].0, "gen_t UPDATE");
    assert!(
        docs[1]
            .1
            .contains("SET\n  \"name\" = NULL\nWHERE \"id\" = NULL;"),
        "{}",
        docs[1].1
    );
    cx.update(|cx| {
        let m = f.model.read(cx);
        assert!(m.query_open);
        assert_eq!(m.active_tab().unwrap().current_query, 1);
    });
}

#[gpui_kit::test]
fn query_tabs_are_added_switched_and_closed(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    with_window(cx, &f, |window, cx| {
        f.model.update(cx, |m, cx| {
            let tab = m.active.unwrap();
            m.add_query(tab, None, Some("select 2".into()), window, cx);
            m.select_query(tab, 0, window, cx);
        })
    });
    let titles: Vec<_> = docs(cx, &f).into_iter().map(|d| d.0).collect();
    assert_eq!(titles, ["クエリ 1", "クエリ 2"]);
    cx.update(|cx| assert_eq!(f.model.read(cx).active_tab().unwrap().current_query, 0));
    cx.update(|cx| {
        f.model.update(cx, |m, cx| {
            let tab = m.active.unwrap();
            m.close_query_doc(tab, 0, cx);
            m.close_query_doc(tab, 0, cx); // 最後の 1 つは閉じられない
        })
    });
    assert_eq!(
        docs(cx, &f),
        [("クエリ 2".to_string(), "select 2".to_string())]
    );
}

#[gpui_kit::test]
fn opening_a_table_with_a_limit_loads_only_that_many_rows(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create table lim_t (id int primary key); insert into lim_t select generate_series(1, 30)",
    );
    cx.update(|cx| {
        f.model.update(cx, |m, _| {
            let tab = m.active.unwrap();
            m.open_table(tab, TableRef::new("public", "lim_t"), Some(10))
        })
    });
    wait_until(cx, "the limited table", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .and_then(|t| t.active_result())
                .is_some_and(|r| r.handle.is_finished() && r.rows == 10)
        })
    });
}

#[gpui_kit::test]
fn viewing_100_rows_shows_the_same_select_in_a_new_tab(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "create table view_t (id int primary key)");
    cx.update(|cx| {
        f.model.update(cx, |m, _| {
            let tab = m.active.unwrap();
            m.view_table(tab, TableRef::new("public", "view_t"), Some(100))
        })
    });
    wait_until(cx, "the SELECT tab", |cx| docs(cx, &f).len() == 2);
    let docs = docs(cx, &f);
    assert_eq!(docs[1].0, "view_t SELECT");
    assert!(docs[1].1.contains("LIMIT 100"), "{}", docs[1].1);
}
