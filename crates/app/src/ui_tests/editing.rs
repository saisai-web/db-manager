//! テーブル編集の画面操作: 開く、セルを編集、保存(プレビュー → 確定)。

use gpui_kit::TestAppContext;
use gpui_kit::component::input::InputContent;
use gpui_kit::test::TestWindowExt as _;
use sqltool_core::TableRef;

use super::harness::*;

fn click(cx: &mut TestAppContext, f: &Fixture, id: &'static str) {
    with_window(cx, f, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
}

fn active_table(
    cx: &mut TestAppContext,
    f: &Fixture,
) -> gpui_kit::Entity<gpui_kit::component::table::TableState<crate::ui::GridDelegate>> {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .active_result()
            .unwrap()
            .table
            .clone()
    })
}

fn select_cell(cx: &mut TestAppContext, f: &Fixture, row: usize, col: usize) {
    let table = active_table(cx, f);
    with_window(cx, f, |_, cx| {
        table.update(cx, |t, cx| t.set_selected_cell(row, col, cx))
    });
}

fn active_edit_summary(cx: &mut TestAppContext, f: &Fixture) -> Option<(usize, usize, usize)> {
    cx.update(|cx| {
        let t = f.model.read(cx).active_tab()?;
        let e = t.active_result()?.edit.as_ref()?;
        Some(e.buffer.borrow().counts())
    })
}

#[gpui_kit::test]
fn editing_a_cell_and_saving_goes_through_preview_and_changes_the_database(
    cx: &mut TestAppContext,
) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create table people (id int primary key, name text); insert into people values (1, 'Alice'), (2, 'Bob')",
    );

    // スキーマツリーの「開く」と同じ操作
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, _| {
            m.open_table(tab, TableRef::new("public", "people"), None)
        });
    });
    wait_until(cx, "the table to open", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .and_then(|t| t.active_result())
                .is_some_and(|r| r.edit.is_some() && r.handle.is_finished())
        })
    });

    // 2 行目(Bob)の name を選び、値を変えて適用する
    select_cell(cx, &f, 1, 1);
    wait_until(cx, "the editor input to show the cell value", |cx| {
        cx.update(|cx| {
            let e = f
                .model
                .read(cx)
                .active_tab()
                .unwrap()
                .active_result()
                .unwrap()
                .edit
                .as_ref()
                .unwrap();
            e.input.read(cx).value() == "Bob"
        })
    });
    let input = cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .unwrap()
            .active_result()
            .unwrap()
            .edit
            .as_ref()
            .unwrap()
            .input
            .clone()
    });
    with_window(cx, &f, |window, cx| {
        input.update(cx, |i, cx| {
            i.set_value(InputContent::from("Bobby 日本語".to_string()), window, cx)
        })
    });
    click(cx, &f, "cell-apply");
    wait_until(cx, "the edit to be buffered", |cx| {
        active_edit_summary(cx, &f) == Some((1, 0, 0))
    });

    // 保存 → プレビューが出る。まだ DB は変わっていない
    click(cx, &f, "edit-save");
    wait_until(cx, "the preview", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .unwrap()
                .edit_preview
                .is_some()
        })
    });
    cx.update(|cx| {
        let p = f
            .model
            .read(cx)
            .active_tab()
            .unwrap()
            .edit_preview
            .as_ref()
            .unwrap();
        assert_eq!(p.items.len(), 1);
        assert!(
            p.items[0].sql.contains("SET \"name\" = 'Bobby 日本語'"),
            "{}",
            p.items[0].sql
        );
        assert!(
            p.items[0]
                .sql
                .contains("\"name\" IS NOT DISTINCT FROM 'Bob'"),
            "{}",
            p.items[0].sql
        );
    });

    // 確定 → 保存され、テーブルが開き直されて、新しい値が見える
    click(cx, &f, "preview-commit");
    wait_until(cx, "the table to reload with the saved value", |cx| {
        cx.update(|cx| {
            let Some(r) = f
                .model
                .read(cx)
                .active_tab()
                .and_then(|t| t.active_result())
            else {
                return false;
            };
            r.handle.is_finished()
                && r.handle.row_count() == 2
                && r.handle
                    .read_row_blocking(1)
                    .ok()
                    .flatten()
                    .is_some_and(|row| row[1].as_deref() == Some("Bobby 日本語".as_bytes()))
        })
    });
    assert_eq!(
        active_edit_summary(cx, &f),
        Some((0, 0, 0)),
        "the reloaded table starts clean"
    );
}

#[gpui_kit::test]
fn cancelling_the_preview_saves_nothing(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create table t (id int primary key, v text); insert into t values (1, 'x')",
    );
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, _| {
            m.open_table(tab, TableRef::new("public", "t"), None)
        });
    });
    wait_until(cx, "the table", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .and_then(|t| t.active_result())
                .is_some_and(|r| r.edit.is_some())
        })
    });
    // 行を削除して、保存 → キャンセル
    select_cell(cx, &f, 0, 0);
    wait_until(cx, "selection", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .unwrap()
                .active_result()
                .unwrap()
                .edit
                .as_ref()
                .unwrap()
                .cell
                .is_some()
        })
    });
    click(cx, &f, "row-delete");
    wait_until(cx, "the delete to be buffered", |cx| {
        active_edit_summary(cx, &f) == Some((0, 1, 0))
    });
    click(cx, &f, "edit-save");
    wait_until(cx, "the preview", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .unwrap()
                .edit_preview
                .is_some()
        })
    });
    click(cx, &f, "preview-cancel");
    wait_until(cx, "the preview to close", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .unwrap()
                .edit_preview
                .is_none()
        })
    });
    // 変更は残り(破棄するまで)、DB は変わっていない
    assert_eq!(active_edit_summary(cx, &f), Some((0, 1, 0)));
    run_sql(cx, &f, "select count(*) from t");
    cx.update(|cx| {
        let r = f
            .model
            .read(cx)
            .active_tab()
            .unwrap()
            .active_result()
            .unwrap();
        assert_eq!(
            r.handle.read_row_blocking(0).unwrap().unwrap()[0].as_deref(),
            Some(&b"1"[..])
        );
    });
}
