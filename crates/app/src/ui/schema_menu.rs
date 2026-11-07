//! スキーマツリーの行を右クリックしたときのメニュー。機能ごとに、見出しと区切り線でまとめる。
//!
//! - スキーマの行: 図(設計に取り込む、ER 図)
//! - テーブルの行: 表示(100 行 / 全件)、SQL を作る(SELECT / INSERT / UPDATE / DELETE)、データ(CSV の取り込み)

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use sqltool_core::{SqlKind, TabId, TableRef};

use super::prelude::*;
use crate::model::AppModel;

/// 「100 行を表示」の行数
const PREVIEW_ROWS: u64 = sqltool_core::SELECT_PREVIEW_ROWS;

/// 右クリックされた行が、何を指しているか。
pub struct Target {
    pub tab: TabId,
    /// スキーマの行なら、その名前
    pub schema: Option<String>,
    /// テーブルの行なら、(スキーマ名, 表名)
    pub table: Option<(String, String)>,
    /// INSERT / UPDATE / DELETE や CSV の取り込みができる表(通常のテーブル)か。ビューなどは読み取りだけ
    pub writable: bool,
}

impl Target {
    pub fn has_menu(&self) -> bool {
        self.schema.is_some() || self.table.is_some()
    }
}

pub fn menu(
    model: Entity<AppModel>,
    target: Target,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _, _| {
        let tab = target.tab;
        match (&target.schema, &target.table) {
            (Some(schema), _) => schema_menu(menu, &model, tab, schema),
            (_, Some((schema, name))) => table_menu(
                menu,
                &model,
                tab,
                TableRef::new(schema, name),
                target.writable,
            ),
            _ => menu,
        }
    }
}

fn schema_menu(menu: PopupMenu, model: &Entity<AppModel>, tab: TabId, schema: &str) -> PopupMenu {
    let (m, s) = (model.clone(), schema.to_string());
    let import = PopupMenuItem::new("設計に取り込む").on_click(move |_, window, cx| {
        let s = s.clone();
        m.update(cx, |m, cx| m.import_schema_to_designer(tab, s, window, cx));
    });
    let (m, s) = (model.clone(), schema.to_string());
    let er = PopupMenuItem::new("ER 図").on_click(move |_, _, cx| {
        let s = s.clone();
        m.update(cx, |m, cx| m.show_er(tab, s, cx));
    });
    menu.item(PopupMenuItem::label("図")).item(import).item(er)
}

fn table_menu(
    menu: PopupMenu,
    model: &Entity<AppModel>,
    tab: TabId,
    table: TableRef,
    writable: bool,
) -> PopupMenu {
    let open = |label: &'static str, limit: Option<u64>| {
        let (m, t) = (model.clone(), table.clone());
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let t = t.clone();
            m.update(cx, |m, _| m.view_table(tab, t, limit));
        })
    };
    let sql = |label: &'static str, kind: SqlKind| {
        let (m, t) = (model.clone(), table.clone());
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let t = t.clone();
            m.update(cx, |m, _| m.request_sql(tab, t, kind));
        })
    };
    let mut menu = menu
        .item(PopupMenuItem::label("表示"))
        .item(open("100 行を表示", Some(PREVIEW_ROWS)))
        .item(open("全件を表示", None))
        .item(PopupMenuItem::separator())
        .item(PopupMenuItem::label("SQL を作る"))
        .item(sql("SELECT(取得)", SqlKind::Select));
    if writable {
        menu = menu
            .item(sql("INSERT(追加)", SqlKind::Insert))
            .item(sql("UPDATE(更新)", SqlKind::Update))
            .item(sql("DELETE(削除)", SqlKind::Delete))
            .item(PopupMenuItem::separator())
            .item(PopupMenuItem::label("データ"))
            .item({
                let m = model.clone();
                PopupMenuItem::new("CSV を取り込む")
                    .on_click(move |_, _, cx| pick_and_import(m.clone(), tab, table.clone(), cx))
            });
    }
    menu
}

fn pick_and_import(model: Entity<AppModel>, tab: TabId, table: TableRef, cx: &mut App) {
    let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("取り込む CSV を選択".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        model.update(cx, |m, _| m.start_import(tab, table, path));
    })
    .detach();
}
