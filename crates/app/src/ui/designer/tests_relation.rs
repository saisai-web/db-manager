//! 関係線の詳細ダイアログの画面テスト(線をダブルクリックして設定する)。

use gpui_kit::TestAppContext;
use gpui_kit::component::input::InputContent;
use gpui_kit::test::{ClickOptions, TestWindowExt as _};
use sqltool_core::design::RefAction;

use super::tests::setup;

#[gpui_kit::test]
fn double_clicking_a_relation_line_opens_its_properties(cx: &mut TestAppContext) {
    let f = setup(cx);
    // 1 回のクリックでは、開かない
    f.click(cx, ("rel", 1usize));
    f.read(cx, |d| assert!(d.relation.is_none()));

    f.with(cx, |window, cx| {
        window.click_with_options(("rel", 1usize), ClickOptions::new().with_count(2), cx);
        window.render_frame(cx);
    });
    f.read(cx, |d| {
        let r = d.relation.as_ref().expect("opened");
        assert_eq!((r.table, r.fk), (1, 0));
    });
    f.render(cx);

    // サンプルは、親が必須で、削除時は CASCADE。削除時の動きを SET NULL(4 番目)にする
    f.click(cx, ("r-del", 3usize));
    f.read(cx, |d| {
        assert_eq!(
            d.design.tables[1].foreign_keys[0].on_delete,
            RefAction::SetNull
        );
        assert!(d.dirty);
    });
    f.click(cx, ("r-upd", 1usize));
    f.read(cx, |d| {
        assert_eq!(
            d.design.tables[1].foreign_keys[0].on_update,
            RefAction::Restrict
        );
    });

    // 親を任意に・1 対 1 に
    f.click(cx, "r-req");
    f.click(cx, "r-one");
    f.read(cx, |d| {
        let t = &d.design.tables[1];
        let fk = &t.foreign_keys[0];
        assert!(!t.fk_required(fk) && t.fk_is_one_to_one(fk));
    });
    f.click(cx, "r-one");
    f.read(cx, |d| {
        let t = &d.design.tables[1];
        assert!(!t.fk_is_one_to_one(&t.foreign_keys[0]));
    });

    // 制約名
    let name = f.read(cx, |d| d.relation.as_ref().unwrap().name.clone());
    f.with(cx, |window, cx| {
        name.update(cx, |s, cx| {
            s.set_value(
                InputContent::from("fk_orders_customer".to_string()),
                window,
                cx,
            )
        });
        f.designer
            .update(cx, |d, cx| d.relation_name_changed(window, cx));
    });
    f.read(cx, |d| {
        assert_eq!(
            d.design.tables[1].foreign_keys[0].name,
            "fk_orders_customer"
        )
    });

    // 閉じる → 消す
    f.click(cx, ("relation-dialog", 1usize));
    f.read(cx, |d| assert!(d.relation.is_none()));
    f.with(cx, |window, cx| {
        window.click_with_options(("rel", 1usize), ClickOptions::new().with_count(2), cx);
        window.render_frame(cx);
    });
    f.render(cx);
    f.click(cx, "r-drop");
    f.read(cx, |d| {
        assert!(d.relation.is_none());
        assert!(d.design.tables[1].foreign_keys.is_empty());
        assert!(d.design.tables[1].column("customer_id").is_some());
    });
}
