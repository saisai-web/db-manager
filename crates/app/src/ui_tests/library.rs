//! 履歴とスニペット: 一覧、エディタへの挿入、保存、削除。

use gpui_kit::TestAppContext;
use gpui_kit::component::input::InputContent;

use super::harness::*;
use crate::model::Panel;

fn editor_text(cx: &mut TestAppContext, f: &Fixture) -> String {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .editor
            .read(cx)
            .value()
            .to_string()
    })
}

fn open_panel(cx: &mut TestAppContext, f: &Fixture, panel: Panel) {
    with_window(cx, f, |window, cx| {
        f.model.update(cx, |m, cx| m.open_panel(panel, window, cx))
    });
}

#[gpui_kit::test]
fn history_lists_executed_statements_and_inserting_appends_to_the_editor(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "select 'first-statement'");
    run_sql(cx, &f, "select 'second-statement'");
    open_panel(cx, &f, Panel::History);
    wait_until(cx, "the history to load", |cx| {
        cx.update(|cx| f.model.read(cx).history.len() >= 2)
    });
    let first = cx.update(|cx| f.model.read(cx).history[0].sql_text.clone());
    assert!(first.contains("second-statement"), "newest first: {first}");

    // 書きかけの SQL を消さずに、末尾に足す。パネルは閉じる
    with_window(cx, &f, |window, cx| {
        let tab = f.model.read(cx).active.unwrap();
        let editor = f.model.read(cx).active_tab().unwrap().editor.clone();
        editor.update(cx, |e, cx| {
            e.set_value(InputContent::from("select 1".to_string()), window, cx)
        });
        f.model.update(cx, |m, cx| {
            m.insert_into_editor(tab, "select 'from history'", window, cx)
        });
    });
    assert_eq!(editor_text(cx, &f), "select 1\n\nselect 'from history'");
    cx.update(|cx| assert!(f.model.read(cx).panel.is_none()));
}

#[gpui_kit::test]
fn snippets_are_saved_from_the_editor_listed_and_deleted(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    open_panel(cx, &f, Panel::Snippets);
    with_window(cx, &f, |window, cx| {
        let tab = f.model.read(cx).active.unwrap();
        let editor = f.model.read(cx).active_tab().unwrap().editor.clone();
        editor.update(cx, |e, cx| {
            e.set_value(
                InputContent::from("select * from t".to_string()),
                window,
                cx,
            )
        });
        let input = f.model.read(cx).library_input.clone().unwrap();
        input.update(cx, |i, cx| {
            i.set_value(InputContent::from("すべて表示".to_string()), window, cx)
        });
        f.model
            .update(cx, |m, cx| m.save_snippet_from_editor(tab, cx));
    });
    wait_until(cx, "the snippet to be listed", |cx| {
        cx.update(|cx| f.model.read(cx).snippets.len() == 1)
    });
    let (name, sql, id) = cx.update(|cx| {
        let s = &f.model.read(cx).snippets[0];
        (s.name.clone(), s.sql_text.clone(), s.id.clone())
    });
    assert_eq!(
        (name.as_str(), sql.as_str()),
        ("すべて表示", "select * from t")
    );

    cx.update(|cx| f.model.update(cx, |m, _| m.delete_snippet(id)));
    wait_until(cx, "the snippet to be removed", |cx| {
        cx.update(|cx| f.model.read(cx).snippets.is_empty())
    });
}
