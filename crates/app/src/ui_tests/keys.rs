//! キー操作: Cmd/Ctrl + Enter でカーソル位置の文(選択範囲があればその範囲)を実行する。

use gpui_kit::component::input::{InputContent, Position};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use super::harness::*;

const TWO_STATEMENTS: &str = "select 1;\nselect 22";

/// 実行のキーの修飾キーは、macOS では Cmd、それ以外では Ctrl(`actions.rs` と同じ)。
fn chord(key: &str) -> String {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    format!("{modifier}-{key}")
}

/// エディタに SQL を入れ、カーソルを `line` 行目(0 始まり)の 2 文字目に置いて、キーを押す。
fn type_and_press(cx: &mut TestAppContext, f: &Fixture, sql: &str, line: u32, key: &str) {
    open_query_palette(cx, f);
    cx.update_window(f.window.into(), |_, window, cx| {
        let editor = f.model.read(cx).active_tab().unwrap().editor().clone();
        editor.update(cx, |e, cx| {
            e.set_value(InputContent::from(sql.to_string()), window, cx);
            e.set_cursor_position(Position::new(line, 2), window, cx);
        });
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
}

fn editor_text(cx: &mut TestAppContext, f: &Fixture) -> String {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .editor()
            .read(cx)
            .value()
            .to_string()
    })
}

fn result_cells(cx: &mut TestAppContext, f: &Fixture) -> Vec<String> {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .results
            .iter()
            .map(|r| {
                let row = r.handle.read_row_blocking(0).unwrap().unwrap();
                String::from_utf8(row[0].clone().unwrap()).unwrap()
            })
            .collect()
    })
}

/// 結果がまだ入れ替わっていない間は、空や古い値を返す(待つ側が繰り返し確かめる)。
fn result_cells_now(cx: &mut TestAppContext, f: &Fixture) -> Vec<String> {
    let has = cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .is_some_and(|t| t.is_idle() && !t.results.is_empty())
    });
    if has { result_cells(cx, f) } else { Vec::new() }
}

fn wait_for_results(cx: &mut TestAppContext, f: &Fixture, n: usize) {
    wait_until(cx, "results", |cx| {
        cx.update(|cx| {
            let t = f.model.read(cx).active_tab().unwrap();
            t.results.len() >= n && t.is_idle()
        })
    });
}

#[gpui_kit::test]
fn cmd_enter_runs_only_the_statement_at_the_cursor_and_inserts_no_newline(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    type_and_press(cx, &f, TWO_STATEMENTS, 1, &chord("enter"));
    wait_for_results(cx, &f, 1);
    assert_eq!(
        result_cells(cx, &f),
        ["22"],
        "the statement under the cursor (line 2)"
    );
    assert_eq!(
        editor_text(cx, &f),
        TWO_STATEMENTS,
        "the shortcut must not type a newline"
    );

    // カーソルが 1 行目にあれば、1 文目が実行される
    type_and_press(cx, &f, TWO_STATEMENTS, 0, &chord("enter"));
    wait_until(cx, "the first statement's result", |cx| {
        result_cells_now(cx, &f) == ["1"]
    });
    assert_eq!(editor_text(cx, &f), TWO_STATEMENTS);
}

#[gpui_kit::test]
fn the_shortcut_respects_read_only_mode_on_production(cx: &mut TestAppContext) {
    let Some(f) = open(cx, true) else { return };
    wait_for_tab(cx, &f);
    type_and_press(
        cx,
        &f,
        "create table must_not_exist (a int)",
        0,
        &chord("enter"),
    );
    wait_until(cx, "the refusal", |cx| {
        cx.update(|cx| {
            let t = f.model.read(cx).active_tab().unwrap();
            t.status.text.contains("読み取り専用") && t.is_idle()
        })
    });
    cx.update(|cx| {
        let t = f.model.read(cx).active_tab().unwrap();
        assert!(t.read_only && t.is_production());
        assert!(t.results.is_empty());
    });
}
