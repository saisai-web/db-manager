//! 実行計画の表示。

use gpui_kit::TestAppContext;
use gpui_kit::component::input::InputContent;

use super::harness::*;

#[gpui_kit::test]
fn explaining_the_statement_under_the_cursor_shows_a_plan_tree(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create table t (a int primary key); insert into t select g from generate_series(1, 500) g; analyze t",
    );
    with_window(cx, &f, |window, cx| {
        let tab = f.model.read(cx).active.unwrap();
        let editor = f.model.read(cx).active_tab().unwrap().editor.clone();
        editor.update(cx, |e, cx| {
            e.set_value(
                InputContent::from("select * from t where a = 7".to_string()),
                window,
                cx,
            )
        });
        f.model.update(cx, |m, cx| m.explain(tab, false, cx));
    });
    wait_until(cx, "the plan", |cx| {
        cx.update(|cx| f.model.read(cx).active_tab().unwrap().explain.is_some())
    });
    cx.update(|cx| {
        let e = f
            .model
            .read(cx)
            .active_tab()
            .unwrap()
            .explain
            .as_ref()
            .unwrap();
        assert!(
            !e.analyze && e.plan.root.kind.contains("Scan"),
            "{:?}",
            e.plan.root.kind
        );
        assert!(e.sql.contains("where a = 7"));
    });

    // 閉じられる
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, cx| m.close_explain(tab, cx));
        assert!(f.model.read(cx).active_tab().unwrap().explain.is_none());
    });
}
