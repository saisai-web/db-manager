//! 左側のスキーマツリー。行の組み立ては `schema_rows`、ここは描画だけ。

use super::prelude::*;
use gpui_kit::component::menu::ContextMenuExt as _;
use sqltool_core::TableKind;

use super::schema_menu::{Target, menu};
use super::schema_rows::{RowKind, TreeRow, build};
use super::workspace::Workspace;

/// 取り込める行(通常のテーブル)か。
fn editable_kind(kind: &RowKind) -> bool {
    matches!(kind, RowKind::Table(TableKind::Table))
}

fn glyph(row: &TreeRow) -> &'static str {
    match (&row.kind, row.key.is_some(), row.expanded) {
        (RowKind::Schema { .. } | RowKind::Table(_), true, true) => "▾",
        (RowKind::Schema { .. } | RowKind::Table(_), true, false) => "▸",
        (RowKind::Column { primary: true }, ..) => "🔑",
        _ => " ",
    }
}

fn kind_mark(kind: &RowKind) -> &'static str {
    match kind {
        RowKind::Table(TableKind::View) => "ビュー",
        RowKind::Table(TableKind::MaterializedView) => "実体化ビュー",
        RowKind::Table(TableKind::ForeignTable) => "外部表",
        _ => "",
    }
}

pub fn schema_tree(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let theme = cx.theme().clone();
    let model = ws.model.read(cx);
    let Some(tab) = model.active_tab() else {
        return div()
            .p_3()
            .text_color(theme.muted_foreground)
            .child("接続すると、スキーマが表示されます")
            .into_any_element();
    };
    let conn = tab.connection.id.clone();
    let tab_id = tab.id;
    let model_handle = ws.model.clone();
    let rows = model.schemas.get(&conn).map(build).unwrap_or_default();

    let items: Vec<AnyElement> = rows
        .into_iter()
        .enumerate()
        .map(|(ix, row)| {
            let muted = matches!(row.kind, RowKind::Note | RowKind::Schema { system: true });
            let key = row.key.clone();
            let line = div()
                .id(("tree-row", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(22.))
                .pl(px(8. + 14. * row.depth as f32))
                .pr_2()
                .text_sm()
                .when(muted, |d| d.text_color(theme.muted_foreground))
                .when(key.is_some(), |d| {
                    d.cursor_pointer().hover(|h| h.bg(theme.accent))
                })
                .child(div().w(px(14.)).text_xs().child(glyph(&row)))
                .child(div().whitespace_nowrap().child(row.label.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .whitespace_nowrap()
                        .child(if row.detail.is_empty() {
                            kind_mark(&row.kind).to_string()
                        } else {
                            row.detail.clone()
                        }),
                )
                .when_some(key, |d, key| {
                    let conn = conn.clone();
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        this.model
                            .update(cx, |m, cx| m.toggle_node(&conn, &key, cx));
                    }))
                });
            let target = Target {
                tab: tab_id,
                schema: matches!(row.kind, RowKind::Schema { .. }).then(|| row.label.to_string()),
                writable: editable_kind(&row.kind),
                table: row.table.clone(),
            };
            if target.has_menu() {
                line.context_menu(menu(model_handle.clone(), target))
                    .into_any_element()
            } else {
                line.into_any_element()
            }
        })
        .collect();

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .h(px(32.))
                .px_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tab.connection.name.clone()),
                )
                .child(
                    Button::new("refresh-schema")
                        .label("更新")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = this
                                .model
                                .read(cx)
                                .active_tab()
                                .map(|t| t.connection.id.clone());
                            if let Some(id) = id {
                                this.model.update(cx, |m, _| m.refresh_schema(id));
                            }
                        })),
                ),
        )
        .child(
            div()
                .id("tree-scroll")
                .flex_1()
                .overflow_y_scroll()
                .children(items),
        )
        .into_any_element()
}
