//! ER 図の表示。実際のデータベースの外部キーから、表と関係が出ることを確かめる。

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

use super::harness::*;
use crate::model::graph_of;

#[gpui_kit::test]
fn the_er_view_shows_tables_and_foreign_keys_of_a_schema(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(
        cx,
        &f,
        "create schema shop; \
         create table shop.customers (id int primary key, name text); \
         create table shop.orders (id int primary key, customer_id int references shop.customers(id)); \
         create table shop.items (order_id int references shop.orders(id), sku text)",
    );
    // DDL の後は、スキーマ情報が読み直される。表の列が届くまで待つ
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model
            .update(cx, |m, cx| m.show_er(tab, "shop".into(), cx));
    });
    wait_until(cx, "all three tables to be loaded", |cx| {
        cx.update(|cx| {
            let m = f.model.read(cx);
            let t = m.active_tab().unwrap();
            m.schemas
                .get(&t.connection.id)
                .is_some_and(|s| graph_of(s, "shop").1.loaded == 3)
        })
    });
    cx.update(|cx| {
        let m = f.model.read(cx);
        let t = m.active_tab().unwrap();
        assert_eq!(t.er.as_deref(), Some("shop"));
        let (g, p) = graph_of(m.schemas.get(&t.connection.id).unwrap(), "shop");
        assert_eq!((p.loaded, p.total), (3, 3));
        let rel: Vec<_> = g
            .relations
            .iter()
            .map(|r| (g.tables[r.from].name.as_str(), g.tables[r.to].name.as_str()))
            .collect();
        assert_eq!(rel, [("items", "orders"), ("orders", "customers")]);
    });

    // 画面に描ける(描画で panic しない)。閉じられる
    with_window(cx, &f, |window, cx| window.render_frame(cx));
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, cx| m.close_er(tab, cx));
        assert!(f.model.read(cx).active_tab().unwrap().er.is_none());
    });
}
