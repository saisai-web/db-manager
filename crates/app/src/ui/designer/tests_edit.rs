//! 図の上でのその場編集・表の詳細エディタ・表の大きさ・ズームの画面テスト(本物のマウス操作)。

use gpui_kit::component::input::{InputContent, InputState};
use gpui_kit::test::ClickOptions;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, point, px};
use sqltool_core::design::DataType;

use super::inline::Field;
use super::tests::{Fixture, setup};
use super::type_menu::position_of;

impl Fixture {
    /// 入力欄の中身を書き換える(利用者が打ったのと同じ結果)。
    fn set(&self, cx: &mut TestAppContext, input: Entity<InputState>, text: &str) {
        self.with(cx, |window, cx| {
            input.update(cx, |s, cx| {
                s.set_value(InputContent::from(text.to_string()), window, cx)
            });
        });
    }

    fn inline_input(&self, cx: &mut TestAppContext) -> Entity<InputState> {
        self.read(cx, |d| d.inline.as_ref().expect("editing").input.clone())
    }

    /// Enter かフォーカス外れと同じ、確定の経路。
    fn commit(&self, cx: &mut TestAppContext) {
        self.with(cx, |window, cx| {
            self.designer
                .update(cx, |d, cx| d.commit_inline(window, cx))
        });
        self.render(cx);
    }

    fn change_detail(&self, cx: &mut TestAppContext) {
        self.with(cx, |window, cx| {
            self.designer
                .update(cx, |d, cx| d.detail_changed(window, cx))
        });
        self.render(cx);
    }
}

#[gpui_kit::test]
fn clicking_a_column_name_edits_it_in_place_and_references_follow(cx: &mut TestAppContext) {
    let f = setup(cx);
    // 顧客の主キー id(0 番目の表の 0 列目)
    f.click(cx, ("cn", 0usize));
    f.read(cx, |d| {
        let i = d.inline.as_ref().expect("click starts editing");
        assert!(i.is(0, Some(0), Field::Name));
    });
    let input = f.inline_input(cx);
    f.set(cx, input, "customer_id");
    f.commit(cx);
    f.read(cx, |d| {
        assert!(d.inline.is_none());
        assert_eq!(d.design.tables[0].columns[0].name, "customer_id");
        // 参照している orders.customer_id → customers の列も、追従する
        assert_eq!(
            d.design.tables[1].foreign_keys[0].ref_columns,
            ["customer_id"]
        );
        assert!(d.dirty);
    });
    f.click(cx, "d-undo");
    f.read(cx, |d| assert_eq!(d.design.tables[0].columns[0].name, "id"));
}

#[gpui_kit::test]
fn clicking_a_type_opens_a_list_to_choose_from_and_a_bad_rename_is_refused(
    cx: &mut TestAppContext,
) {
    let f = setup(cx);
    f.click(cx, ("cty", 1usize)); // customers.name の型
    f.read(cx, |d| {
        assert!(
            d.type_menu
                .as_ref()
                .is_some_and(|m| (m.table, m.col) == (0, 1))
        );
        assert!(d.inline.is_none(), "a list, not a text box");
    });
    f.render(cx);
    f.click(cx, ("ty", position_of("varchar(50)").unwrap()));
    f.read(cx, |d| {
        assert!(d.type_menu.is_none());
        assert_eq!(
            d.design.tables[0].columns[1].ty,
            DataType::Varchar(Some(50))
        );
    });
    // 一覧にない型は、直接入力
    f.click(cx, ("cty", 1usize));
    f.click(cx, "ty-other");
    f.read(cx, |d| {
        assert!(
            d.inline
                .as_ref()
                .is_some_and(|i| i.is(0, Some(1), Field::Type))
        )
    });
    let input = f.inline_input(cx);
    f.set(cx, input, "varchar(30)");
    f.commit(cx);
    f.read(cx, |d| {
        assert_eq!(
            d.design.tables[0].columns[1].ty,
            DataType::Varchar(Some(30))
        )
    });
    // 何もない所をクリックすると、一覧は閉じる
    f.click(cx, ("cty", 1usize));
    f.with(cx, |window, cx| {
        f.designer.update(cx, |d, cx| d.close_type_menu(cx));
        window.render_frame(cx);
    });
    f.read(cx, |d| assert!(d.type_menu.is_none()));

    f.click(cx, ("cn", 1usize));
    let input = f.inline_input(cx);
    f.set(cx, input, "email"); // 同じ表に、すでにある名前
    f.commit(cx);
    f.read(cx, |d| {
        assert_eq!(d.design.tables[0].columns[1].name, "name");
        assert!(
            d.message
                .as_ref()
                .is_some_and(|m| m.0 && m.1.contains("すでに"))
        );
    });
}

#[gpui_kit::test]
fn tab_confirms_and_moves_to_the_next_column_and_escape_discards(cx: &mut TestAppContext) {
    let f = setup(cx);
    let key = |f: &Fixture, cx: &mut TestAppContext, k: &'static str, shift: bool| {
        f.with(cx, |window, cx| {
            f.designer
                .update(cx, |d, cx| d.inline_key(k, shift, window, cx))
        });
        f.render(cx);
    };
    f.click(cx, ("cn", 0usize));
    let input = f.inline_input(cx);
    f.set(cx, input, "pk");
    key(&f, cx, "tab", false);
    f.read(cx, |d| {
        assert_eq!(d.design.tables[0].columns[0].name, "pk");
        assert!(
            d.inline
                .as_ref()
                .is_some_and(|i| i.is(0, Some(1), Field::Name))
        );
    });
    key(&f, cx, "tab", true);
    f.read(cx, |d| {
        assert!(
            d.inline
                .as_ref()
                .is_some_and(|i| i.is(0, Some(0), Field::Name))
        )
    });
    let input = f.inline_input(cx);
    f.set(cx, input, "discarded");
    key(&f, cx, "escape", false);
    f.read(cx, |d| {
        assert!(d.inline.is_none());
        assert_eq!(d.design.tables[0].columns[0].name, "pk");
    });
    // 最後の列で Tab を押すと、確定して終わる
    f.click(cx, ("cn", 2usize));
    key(&f, cx, "tab", false);
    f.read(cx, |d| assert!(d.inline.is_none()));
}

#[gpui_kit::test]
fn an_unchanged_edit_leaves_no_history_and_the_header_edits_the_table_name(
    cx: &mut TestAppContext,
) {
    let f = setup(cx);
    f.click(cx, ("cn", 0usize));
    f.commit(cx);
    f.read(cx, |d| assert!(d.undo.is_empty() && !d.dirty));

    f.click(cx, ("tn", 1usize));
    f.read(cx, |d| {
        assert!(
            d.inline
                .as_ref()
                .is_some_and(|i| i.is(1, None, Field::Name))
        )
    });
    let input = f.inline_input(cx);
    f.set(cx, input, "purchases");
    f.commit(cx);
    f.read(cx, |d| assert_eq!(d.design.tables[1].name, "purchases"));
}

#[gpui_kit::test]
fn the_plus_button_adds_a_column_and_starts_editing_its_name(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.click(cx, ("t-add", 0usize));
    f.read(cx, |d| {
        let t = &d.design.tables[0];
        assert_eq!(t.columns.len(), 4);
        assert_eq!(t.columns[3].name, "column");
        assert!(
            d.inline
                .as_ref()
                .is_some_and(|i| i.is(0, Some(3), Field::Name))
        );
    });
    let input = f.inline_input(cx);
    f.set(cx, input, "phone");
    f.commit(cx);
    f.read(cx, |d| {
        assert_eq!(d.design.tables[0].columns[3].name, "phone")
    });
    f.click(cx, "d-undo");
    f.click(cx, "d-undo");
    f.read(cx, |d| assert_eq!(d.design.tables[0].columns.len(), 3));
}

#[gpui_kit::test]
fn double_clicking_a_table_opens_the_detail_editor_where_everything_can_change(
    cx: &mut TestAppContext,
) {
    let f = setup(cx);
    let at = f.center(cx, ("th", 1usize));
    f.with(cx, |window, cx| {
        window.click_with_options(("th", 1usize), ClickOptions::new().with_count(2), cx);
        window.render_frame(cx);
    });
    let _ = at;
    f.read(cx, |d| {
        assert!(d.detail.as_ref().is_some_and(|x| x.table == 1))
    });
    f.render(cx);

    // 表の名前・論理名と、列の名前・型・既定値
    let (name, logical, col_name, col_ty, col_default) = f.read(cx, |d| {
        let x = d.detail.as_ref().unwrap();
        (
            x.name.clone(),
            x.logical.clone(),
            x.cols[1].name.clone(),
            x.cols[1].ty.clone(),
            x.cols[1].default.clone(),
        )
    });
    f.set(cx, name, "purchases");
    f.set(cx, logical, "購入");
    f.set(cx, col_name, "buyer_id");
    f.set(cx, col_ty, "bigint");
    f.set(cx, col_default, "0");
    f.change_detail(cx);
    f.read(cx, |d| {
        let t = &d.design.tables[1];
        assert_eq!((t.name.as_str(), t.logical.as_str()), ("purchases", "購入"));
        assert_eq!(t.columns[1].name, "buyer_id");
        assert_eq!(t.columns[1].ty, DataType::BigInt);
        assert_eq!(t.columns[1].default.as_deref(), Some("0"));
        assert_eq!(
            t.foreign_keys[0].columns,
            ["buyer_id"],
            "the relation follows"
        );
    });

    // 制約の切り替え・列の追加と削除・並べ替え
    f.click(cx, ("d-uq", 1usize));
    f.read(cx, |d| assert!(d.design.tables[1].columns[1].unique));
    f.click(cx, ("d-nn", 1usize));
    f.read(cx, |d| assert!(d.design.tables[1].columns[1].nullable));
    f.click(cx, ("d-pk", 2usize));
    f.read(cx, |d| {
        let c = &d.design.tables[1].columns[2];
        assert!(c.primary && !c.nullable);
    });
    f.click(cx, "d-add-col");
    f.read(cx, |d| {
        assert_eq!(d.design.tables[1].columns.len(), 4);
        assert_eq!(d.detail.as_ref().unwrap().cols.len(), 4, "inputs follow");
    });
    f.click(cx, ("d-up", 3usize));
    f.read(cx, |d| {
        assert_eq!(d.design.tables[1].columns[2].name, "column")
    });
    f.click(cx, ("d-del", 2usize));
    f.read(cx, |d| assert_eq!(d.design.tables[1].columns.len(), 3));

    // 型は、一覧からも選べる
    f.click(cx, ("d-ty", 1usize));
    f.read(cx, |d| {
        assert!(d.type_menu.as_ref().is_some_and(|m| m.in_dialog))
    });
    f.render(cx);
    f.click(cx, ("ty", position_of("uuid").unwrap()));
    f.read(cx, |d| {
        assert_eq!(d.design.tables[1].columns[1].ty, DataType::Uuid);
        assert!(d.type_menu.is_none() && d.detail.is_some());
    });

    // 表の大きさ
    f.click(cx, "d-bigger");
    f.read(cx, |d| {
        assert!((d.design.tables[1].factor() - 1.1).abs() < 1e-3)
    });
    f.click(cx, "d-reset-size");
    f.read(cx, |d| assert_eq!(d.design.tables[1].scale, None));

    f.click(cx, ("detail-dialog", 1usize));
    f.read(cx, |d| assert!(d.detail.is_none()));
    f.click(cx, "d-undo");
    f.render(cx);
}

#[gpui_kit::test]
fn a_bad_name_in_the_detail_editor_is_refused_and_the_table_can_be_deleted(
    cx: &mut TestAppContext,
) {
    let f = setup(cx);
    f.click(cx, ("t-detail", 1usize));
    let name = f.read(cx, |d| d.detail.as_ref().unwrap().name.clone());
    f.set(cx, name.clone(), "customers"); // すでにある
    f.change_detail(cx);
    f.read(cx, |d| {
        assert_eq!(d.design.tables[1].name, "orders");
        assert!(d.message.as_ref().is_some_and(|m| m.0));
    });
    f.set(cx, name, "has space");
    f.change_detail(cx);
    f.read(cx, |d| assert_eq!(d.design.tables[1].name, "orders"));
    f.click(cx, "d-del-table");
    f.read(cx, |d| {
        assert!(d.detail.is_none());
        assert_eq!(d.design.tables.len(), 1);
    });
}

#[gpui_kit::test]
fn dragging_the_corner_resizes_a_table_and_undo_restores_it(cx: &mut TestAppContext) {
    let f = setup(cx);
    let before = f.read(cx, |d| d.derived.geo.rects[1]);
    let from = f.center(cx, ("grip", 1usize));
    f.with(cx, |window, cx| {
        window.drag(from, point(from.x + px(120.), from.y), cx);
        window.render_frame(cx);
    });
    f.read(cx, |d| {
        let t = &d.design.tables[1];
        assert!(t.factor() > 1.3, "{:?}", t.scale);
        let r = d.derived.geo.rects[1];
        assert!(r.w > before.w * 1.3 && r.h > before.h * 1.3);
        assert!(
            d.design.tables.iter().all(|t| t.pos.is_some()),
            "positions are pinned"
        );
        assert!(
            d.last_text
                .contains(&format!(",{}", (t.factor() * 100.0).round() / 100.0))
        );
    });
    f.click(cx, "d-undo");
    f.read(cx, |d| assert_eq!(d.design.tables[1].scale, None));
}

#[gpui_kit::test]
fn zoom_buttons_reset_and_fit_stay_within_range(cx: &mut TestAppContext) {
    let f = setup(cx);
    for _ in 0..30 {
        f.click(cx, "d-zoom-in");
    }
    f.read(cx, |d| assert_eq!(d.zoom, super::state::MAX_ZOOM));
    for _ in 0..30 {
        f.click(cx, "d-zoom-out");
    }
    f.read(cx, |d| assert_eq!(d.zoom, super::state::MIN_ZOOM));
    f.click(cx, "d-zoom-reset");
    f.read(cx, |d| assert_eq!(d.zoom, 1.0));
    // 大きな図は、「全体」で画面に収まる倍率になる
    let mut text = String::new();
    for i in 0..40 {
        text.push_str(&format!(
            "t{i}\n  id int pk\n  a varchar(10)\n  b varchar(10)\n"
        ));
    }
    f.type_text(cx, &text);
    f.click(cx, "d-zoom-fit");
    f.read(cx, |d| {
        assert!(
            d.zoom < 1.0 && d.zoom >= super::state::MIN_ZOOM,
            "{}",
            d.zoom
        )
    });
    f.render(cx);
}

#[gpui_kit::test]
fn the_text_pane_is_optional_and_comes_back_in_sync(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.click(cx, ("t-add", 0usize)); // テキストを閉じたまま編集する
    f.click(cx, "d-text");
    let (shown, last, text) = cx.update(|cx| {
        let d = f.designer.read(cx);
        (
            d.show_text,
            d.last_text.clone(),
            d.text.read(cx).value().to_string(),
        )
    });
    assert!(shown);
    assert!(last.contains("column"));
    assert_eq!(text, last, "the editor is filled when the pane opens");
    f.click(cx, "d-text");
    f.read(cx, |d| assert!(!d.show_text));
}
