//! SQL 補完: スキーマの先読みと、エディタに設定された補完の提供元。

use gpui_kit::TestAppContext;
use gpui_kit::component::input::{InputContent, RopeExt as _};
use lsp_types::{CompletionContext, CompletionResponse, CompletionTextEdit, CompletionTriggerKind};

use super::harness::*;

/// `|` の位置にカーソルがあるものとして、エディタの補完を呼び、候補のラベルと置き換える範囲の語を返す。
fn complete(cx: &mut TestAppContext, f: &Fixture, sql_with_cursor: &str) -> (Vec<String>, String) {
    let offset = sql_with_cursor.find('|').unwrap();
    let sql = sql_with_cursor.replace('|', "");
    let task = with_window(cx, f, |window, cx| {
        let editor = f.model.read(cx).active_tab().unwrap().editor.clone();
        editor.update(cx, |e, cx| {
            e.set_value(InputContent::from(sql.clone()), window, cx)
        });
        let (provider, rope) = editor.read_with(cx, |e, _| {
            (
                e.lsp()
                    .completion_provider
                    .clone()
                    .expect("a completion provider is installed"),
                e.text().clone(),
            )
        });
        let ctx = CompletionContext {
            trigger_kind: CompletionTriggerKind::INVOKED,
            trigger_character: None,
        };
        provider.completions(&rope, offset, ctx, window, cx)
    });
    let response = futures::executor::block_on(task).expect("completions");
    let CompletionResponse::Array(items) = response else {
        panic!("expected an array")
    };
    let replaced = items
        .first()
        .and_then(|i| match i.text_edit.as_ref() {
            Some(CompletionTextEdit::Edit(e)) => Some(e.range),
            _ => None,
        })
        .map(|r| {
            let rope = gpui_kit::component::Rope::from(sql.as_str());
            sql[rope.position_to_offset(&r.start)..rope.position_to_offset(&r.end)].to_string()
        })
        .unwrap_or_default();
    (items.into_iter().map(|i| i.label).collect(), replaced)
}

fn wait_for_columns(cx: &mut TestAppContext, f: &Fixture, table: &str) {
    wait_until(cx, "the schema to be prefetched", |cx| {
        cx.update(|cx| {
            let m = f.model.read(cx);
            let conn = &m.active_tab().unwrap().connection.id;
            m.catalog_for(conn)
                .tables
                .iter()
                .any(|t| t.name == table && !t.columns.is_empty())
        })
    });
}

#[gpui_kit::test]
fn completion_offers_prefetched_tables_and_columns_without_asking_the_database(
    cx: &mut TestAppContext,
) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create table customers (id int, name text, email text); create table orders (id int, customer_id int, total numeric)",
    );
    wait_for_columns(cx, &f, "customers");
    wait_for_columns(cx, &f, "orders");

    let (c, replaced) = complete(cx, &f, "select * from cus|");
    assert_eq!(c.first().map(String::as_str), Some("customers"), "{c:?}");
    assert_eq!(replaced, "cus");

    let (c, _) = complete(
        cx,
        &f,
        "select c.| from customers c join orders o on o.customer_id = c.id",
    );
    assert_eq!(c, ["email", "id", "name"], "columns of the aliased table");

    let (c, _) = complete(cx, &f, "select name, em| from customers");
    assert_eq!(c.first().map(String::as_str), Some("email"));

    let (c, _) = complete(cx, &f, "sel|");
    assert!(c.contains(&"select".to_string()), "{c:?}");
}

#[gpui_kit::test]
fn a_new_table_shows_up_after_ddl_without_a_manual_refresh(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "create table first_table (a int)");
    wait_for_columns(cx, &f, "first_table");
    run_sql(cx, &f, "create table second_table (b int)");
    wait_for_columns(cx, &f, "second_table");
    let (c, _) = complete(cx, &f, "select * from sec|");
    assert_eq!(c.first().map(String::as_str), Some("second_table"), "{c:?}");
}
