//! ER 設計の画面テスト。ヘッドレスのウィンドウで、本物のマウス・クリックの操作をする(DB は不要)。

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, point, px, size,
};
use sqltool_core::design::{dsl, validate};
use sqltool_core::{Core, CoreConfig, DriverRegistry};

use super::{Designer, SidePanel};
use crate::model::AppModel;
use crate::ui::Workspace;

pub(super) struct Fixture {
    _runtime: tokio::runtime::Runtime,
    pub(super) window: WindowHandle<Root>,
    pub(super) designer: Entity<Designer>,
}

pub(super) fn setup(cx: &mut TestAppContext) -> Fixture {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let config = CoreConfig::new(
        Arc::new(storage::MemorySecretStore::new()),
        DriverRegistry::new(),
    );
    let (core, events) = Core::start(config, runtime.handle().clone()).unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::actions::bind_keys(cx);
    });
    let mut model = None;
    let window = cx.update(|cx| {
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1400.), px(900.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let m = AppModel::start(core, events, window.window_handle(), cx);
                model = Some(m.clone());
                cx.new(|cx| Workspace::new(m, cx))
            },
        )
        .expect("open test window");
        window.downcast::<Root>().expect("Root")
    });
    let model = model.unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        model.update(cx, |m, cx| m.open_designer(None, window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    let designer = cx.update(|cx| model.read(cx).designer.clone().expect("designer"));
    Fixture {
        _runtime: runtime,
        window,
        designer,
    }
}

impl Fixture {
    pub(super) fn with<R>(
        &self,
        cx: &mut TestAppContext,
        body: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(self.window.into(), |_, window, cx| body(window, cx))
            .unwrap()
    }

    pub(super) fn render(&self, cx: &mut TestAppContext) {
        self.with(cx, |window, cx| window.render_frame(cx));
    }

    pub(super) fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&Designer) -> R) -> R {
        cx.update(|cx| f(self.designer.read(cx)))
    }

    pub(super) fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        self.with(cx, |window, cx| {
            let editor = self.designer.read(cx).text.clone();
            editor.update(cx, |e, cx| e.replace_all(text.to_string(), window, cx));
            window.render_frame(cx);
        });
        cx.run_until_parked();
    }

    pub(super) fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
        let id = id.into();
        self.with(cx, |window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
            window.render_frame(cx);
        });
    }

    pub(super) fn center(
        &self,
        cx: &mut TestAppContext,
        id: impl Into<gpui_kit::ElementId>,
    ) -> Point<gpui_kit::Pixels> {
        let id = id.into();
        self.with(cx, |window, cx| {
            window.render_frame(cx);
            window.find(id).bounds().center()
        })
    }
}

#[gpui_kit::test]
fn it_opens_with_a_starter_design_whose_text_and_diagram_agree(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.render(cx);
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 2);
        assert_eq!(d.derived.geo.rects.len(), 2);
        assert_eq!(d.derived.geo.edges.len(), 1);
        assert_eq!(dsl::parse(&d.last_text).unwrap(), d.design);
        assert!(validate(&d.design).is_empty());
        assert!(d.error.is_none());
        // 図だけを出す。テキストも CREATE 文も、開くまでは作らない
        assert!(!d.show_text && !d.export_open && d.derived.sql.is_empty());
    });
    f.click(cx, "d-export");
    f.read(cx, |d| {
        assert!(d.derived.sql.contains("CREATE TABLE customers"))
    });
    f.click(cx, ("export-dialog", 1usize));
    f.read(cx, |d| assert!(!d.export_open && d.derived.sql.is_empty()));
}

#[gpui_kit::test]
fn typing_updates_the_diagram_and_a_mistake_keeps_the_last_good_design(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.type_text(
        cx,
        "a \"甲\"\n  id int pk\nb\n  id int pk\n  a_id int -> a.id\n",
    );
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 2);
        assert_eq!(d.design.tables[0].logical, "甲");
        assert_eq!(d.derived.geo.edges.len(), 1);
        assert!(d.dirty);
    });
    f.click(cx, "d-export");
    f.read(cx, |d| assert!(d.derived.sql.contains("CREATE TABLE a (")));
    f.click(cx, ("export-dialog", 1usize));
    f.type_text(cx, "a\n  id int pk\n  oops\n");
    f.read(cx, |d| {
        assert!(
            d.error.as_deref().is_some_and(|e| e.contains("3 行目")),
            "{:?}",
            d.error
        );
        assert_eq!(
            d.design.tables.len(),
            2,
            "the diagram keeps the last good design"
        );
    });
    f.render(cx);
    // GUI の操作は、テキストの誤りを上書きしないよう、断る
    f.click(cx, "d-add");
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 2);
        assert!(
            d.message
                .as_ref()
                .is_some_and(|m| m.0 && m.1.contains("誤り"))
        );
    });
    // 直せば戻る
    f.type_text(cx, "a\n  id int pk\n");
    f.read(cx, |d| {
        assert!(d.error.is_none());
        assert_eq!(d.design.tables.len(), 1);
    });
}

#[gpui_kit::test]
fn add_table_and_undo_redo_round_trip_through_the_text(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.click(cx, "d-add");
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 3);
        assert!(
            d.last_text.contains("table \"新しい表\""),
            "{}",
            d.last_text
        );
        assert_eq!(d.selected.as_deref(), Some("table"));
    });
    f.click(cx, "d-undo");
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 2);
        assert!(!d.last_text.contains("新しい表"));
    });
    f.click(cx, "d-redo");
    f.read(cx, |d| assert_eq!(d.design.tables.len(), 3));
}

#[gpui_kit::test]
fn dragging_a_table_header_moves_it_and_pins_every_table_where_it_was(cx: &mut TestAppContext) {
    let f = setup(cx);
    let before = f.read(cx, |d| d.derived.geo.rects.clone());
    let from = f.center(cx, ("th", 0usize));
    f.with(cx, |window, cx| {
        window.drag(from, point(from.x + px(103.), from.y + px(57.)), cx);
        window.render_frame(cx);
    });
    f.read(cx, |d| {
        let moved = d.design.tables[0].pos.expect("moved table is pinned");
        let r0 = before[0];
        assert_eq!(
            moved.0,
            (r0.x + 103.0).round(),
            "follows the pointer exactly"
        );
        assert_eq!(moved.1, (r0.y + 57.0).round());
        let other = d.design.tables[1]
            .pos
            .expect("the other table is pinned in place too");
        assert_eq!(other, (before[1].x.round(), before[1].y.round()));
        assert!(d.last_text.contains('@'), "{}", d.last_text);
        assert!(d.dirty);
        assert_eq!((d.derived.geo.rects[0].x, d.derived.geo.rects[0].y), moved);
    });
    // 1 回で元に戻る
    f.click(cx, "d-undo");
    f.read(cx, |d| {
        assert!(d.design.tables.iter().all(|t| t.pos.is_none()))
    });
}

#[gpui_kit::test]
fn a_click_without_movement_only_selects_the_table(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.click(cx, ("th", 1usize));
    f.read(cx, |d| {
        assert_eq!(d.selected.as_deref(), Some("orders"));
        assert!(
            d.design.tables.iter().all(|t| t.pos.is_none()),
            "no pin without a drag"
        );
        assert!(d.undo.is_empty());
    });
}

#[gpui_kit::test]
fn selecting_two_tables_in_connect_mode_adds_the_column_and_the_relation(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.type_text(
        cx,
        "customers\n  id int pk\nproducts \"商品\"\n  id int pk\n",
    );
    f.click(cx, "d-connect");
    f.click(cx, ("th", 1usize)); // products(子)
    f.read(cx, |d| {
        assert!(d.pick.is_some() && d.design.tables[1].foreign_keys.is_empty())
    });
    f.click(cx, ("th", 0usize)); // customers(親)
    f.read(cx, |d| {
        let p = &d.design.tables[1];
        assert_eq!(p.foreign_keys.len(), 1);
        assert!(p.column("customer_id").is_some(), "{}", d.last_text);
        assert!(d.last_text.contains("customer_id int"));
        assert!(d.pick.is_none());
        assert_eq!(d.derived.geo.edges.len(), 1);
    });
}

#[gpui_kit::test]
fn selecting_two_columns_relates_exactly_those_columns(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.type_text(cx, "a\n  id int pk\nb\n  id int pk\n  ref int\n");
    f.click(cx, "d-connect");
    f.click(cx, ("col", 10_000 + 1usize)); // b.ref
    f.click(cx, ("col", 0usize)); // a.id
    f.read(cx, |d| {
        let b = &d.design.tables[1];
        assert_eq!(b.columns.len(), 2, "no column is added");
        assert_eq!(b.foreign_keys[0].columns, ["ref"]);
        assert_eq!(b.foreign_keys[0].ref_columns, ["id"]);
    });
}

#[gpui_kit::test]
fn deleting_a_table_and_laying_out(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.type_text(
        cx,
        "users\n  id int pk\nposts\n  id int pk\n  user_id int\n  @@@\n",
    );
    f.type_text(
        cx,
        "users\n  id int pk\nposts\n  id int pk\n  user_id int -> users.id\n",
    );
    f.read(cx, |d| assert_eq!(d.design.tables[1].foreign_keys.len(), 1));
    f.click(cx, ("th", 0usize));
    f.click(cx, "d-delete");
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 1);
        assert!(
            d.design.tables[0].foreign_keys.is_empty(),
            "relations to the removed table go too"
        );
        assert!(d.selected.is_none());
    });
    f.click(cx, "d-layout");
    f.read(cx, |d| {
        assert!(d.design.tables.iter().all(|t| t.pos.is_none()))
    });
}

#[gpui_kit::test]
fn the_side_panels_and_every_target_render_without_panicking(cx: &mut TestAppContext) {
    use sqltool_core::design::ddl::Target;
    let f = setup(cx);
    f.click(cx, "d-export");
    for t in Target::ALL {
        f.click(cx, ("target", t as usize));
        f.read(cx, |d| {
            assert_eq!(d.target, t);
            assert!(
                d.derived.sql.contains(&format!("({})", t.label())),
                "{}",
                d.derived.sql
            );
        });
    }
    f.click(cx, "o-drop");
    f.read(cx, |d| assert!(d.derived.sql.contains("DROP")));
    for panel in ["p-check", "p-docs", "p-sql"] {
        f.click(cx, panel);
    }
    f.read(cx, |d| assert!(d.panel == SidePanel::Sql));
    f.click(cx, ("export-dialog", 1usize));
    f.click(cx, "d-logical");
    f.click(cx, "d-zoom-out");
    f.click(cx, "d-zoom-in");
    f.read(cx, |d| assert!((d.zoom - 1.0).abs() < 1e-4, "{}", d.zoom));
    f.render(cx);
}

#[gpui_kit::test]
fn closing_hides_the_screen_but_keeps_the_design(cx: &mut TestAppContext) {
    let f = setup(cx);
    f.click(cx, "d-add");
    f.click(cx, "d-close");
    f.read(cx, |d| assert!(!d.visible && d.design.tables.len() == 3));
    f.render(cx);
}

#[gpui_kit::test]
fn files_round_trip_and_a_broken_file_is_refused(cx: &mut TestAppContext) {
    let f = setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shop.erd");
    std::fs::write(&path, "a \"甲\"\n  id int pk\n").unwrap();
    let p = path.clone();
    f.with(cx, |window, cx| {
        f.designer.update(cx, |d, cx| d.open_path(p, window, cx));
    });
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 1);
        assert_eq!(d.file.as_deref(), Some(path.as_path()));
        assert!(!d.dirty);
    });
    f.click(cx, "d-add");
    f.click(cx, "d-save"); // ファイルがあるので、ダイアログなしで上書き
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(dsl::parse(&saved).unwrap().tables.len(), 2, "{saved}");
    f.read(cx, |d| assert!(!d.dirty));

    let bad = dir.path().join("bad.erd");
    std::fs::write(&bad, "a\n  id\n").unwrap();
    f.with(cx, |window, cx| {
        f.designer.update(cx, |d, cx| d.open_path(bad, window, cx));
    });
    f.read(cx, |d| {
        assert_eq!(d.design.tables.len(), 2, "unchanged");
        assert!(d.message.as_ref().is_some_and(|m| m.0));
    });
}

#[gpui_kit::test]
fn a_live_schema_import_replaces_the_design_and_can_be_undone(cx: &mut TestAppContext) {
    use sqltool_core::design::{Column, DataType, Design, Table};
    let f = setup(cx);
    let mut t = Table::new("imported");
    t.columns.push(Column::new("id", DataType::Int));
    let design = Design {
        tables: vec![t],
        ..Default::default()
    };
    f.with(cx, |window, cx| {
        f.designer
            .update(cx, |d, cx| d.import(design, "demo", window, cx));
    });
    f.read(cx, |d| {
        assert_eq!(d.design.tables[0].name, "imported");
        assert!(
            d.message
                .as_ref()
                .is_some_and(|m| m.1.contains("1 表を取り込みました"))
        );
    });
    f.click(cx, "d-undo");
    f.read(cx, |d| assert_eq!(d.design.tables.len(), 2));
}

#[test]
fn every_export_produces_a_file_of_the_right_kind() {
    use super::files::{Export, Job};
    use sqltool_core::design::ddl::{DdlOptions, Target};
    use sqltool_core::design::docs::DocOptions;
    let design = dsl::parse(super::state::TEMPLATE).unwrap();
    let job = |kind| Job {
        kind,
        design: design.clone(),
        target: Target::Postgres,
        ddl: DdlOptions::default(),
        docs: DocOptions {
            ddl: Some(Target::Postgres),
            ..Default::default()
        },
    };
    let head = |kind, n: usize| {
        let b = job(kind).bytes().unwrap();
        String::from_utf8_lossy(&b[..n.min(b.len())]).into_owned()
    };
    assert_eq!(head(Export::Xlsx, 2), "PK");
    assert!(head(Export::Html, 15).starts_with("<!DOCTYPE html>"));
    assert!(head(Export::Svg, 5).starts_with("<?xml"));
    assert!(head(Export::Sql, 40).starts_with("-- 新しい設計(PostgreSQL)"));
    let html = String::from_utf8(job(Export::Html).bytes().unwrap()).unwrap();
    assert!(html.contains("作成日: <b>20"), "the date is filled in");
}

#[gpui_kit::test]
fn a_long_create_script_is_cut_for_display_but_not_for_copy_or_save(cx: &mut TestAppContext) {
    let f = setup(cx);
    let mut text = String::new();
    for i in 0..120 {
        text.push_str(&format!("t{i}\n  id int pk\n  name varchar(10)\n"));
    }
    f.type_text(cx, &text);
    f.click(cx, "d-export");
    f.read(cx, |d| {
        assert!(d.derived.sql.contains("CREATE TABLE t119"));
        assert!(!d.derived.preview.contains("CREATE TABLE t119"));
        assert!(d.derived.preview.contains("省略しています"));
    });
    f.render(cx);
}
