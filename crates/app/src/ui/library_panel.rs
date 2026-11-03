//! 履歴とスニペットのパネル。選んだ SQL は、エディタの末尾に足される。

use gpui_kit::component::input::Input;
use sqltool_core::TabId;

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;
use crate::model::{Panel, preview};

pub fn library_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    // 描画に必要な値を写して、モデルの借用を手放してから、ボタンのリスナーを登録する
    let (panel, tab, input, specs) = {
        let model = ws.model.read(cx);
        let panel = model.panel?;
        let specs: Vec<RowSpec> = match panel {
            Panel::History => model
                .history
                .iter()
                .map(|h| RowSpec {
                    title: preview(&h.sql_text),
                    meta: format!(
                        "{}  {}",
                        h.executed_at
                            .get(..19)
                            .unwrap_or(&h.executed_at)
                            .replace('T', " "),
                        h.duration_ms.map(|d| format!("{d} ms")).unwrap_or_default()
                    ),
                    failed: h.status != sqltool_core::HistoryStatus::Ok,
                    sql: h.sql_text.clone(),
                    snippet_id: None,
                })
                .collect(),
            Panel::Snippets => model
                .snippets
                .iter()
                .map(|s| RowSpec {
                    title: format!("{}:  {}", s.name, preview(&s.sql_text)),
                    meta: s.updated_at.get(..10).unwrap_or("").to_string(),
                    failed: false,
                    sql: s.sql_text.clone(),
                    snippet_id: Some(s.id.clone()),
                })
                .collect(),
        };
        (panel, model.active, model.library_input.clone()?, specs)
    };
    let rows: Vec<AnyElement> = specs
        .into_iter()
        .enumerate()
        .map(|(ix, r)| entry_row(ix, r, tab, cx))
        .collect();
    let empty = rows.is_empty();
    let (title, hint) = match panel {
        Panel::History => ("実行履歴", "SQL の一部で絞り込み(Enter)"),
        Panel::Snippets => ("スニペット", "名前を入力して、エディタの SQL を保存"),
    };
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .child(
                div()
                    .w(px(760.))
                    .h(px(560.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme_colors.border)
                    .bg(theme_colors.background)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().child(Input::new(&input)))
                            .child(match panel {
                                Panel::History => Button::new("library-search")
                                    .label("検索")
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.update(cx, |m, cx| m.search_history(cx));
                                    })),
                                Panel::Snippets => Button::new("library-save")
                                    .label("保存")
                                    .small()
                                    .primary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(tab) = tab {
                                            this.model.update(cx, |m, cx| {
                                                m.save_snippet_from_editor(tab, cx)
                                            });
                                        }
                                    })),
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme_colors.muted_foreground)
                            .child(hint),
                    )
                    .child(
                        div()
                            .id("library-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(rows)
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .p_3()
                                        .text_color(theme_colors.muted_foreground)
                                        .child("まだありません"),
                                )
                            }),
                    )
                    .child(
                        div().flex().flex_row().justify_end().child(
                            Button::new("library-close")
                                .label("閉じる")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.update(cx, |m, cx| m.close_panel(cx));
                                })),
                        ),
                    ),
            ),
    )
}

/// 一覧の 1 行に出す内容。
struct RowSpec {
    title: String,
    meta: String,
    failed: bool,
    sql: String,
    snippet_id: Option<String>,
}

fn entry_row(
    ix: usize,
    spec: RowSpec,
    tab: Option<TabId>,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let RowSpec {
        title,
        meta,
        failed,
        sql,
        snippet_id,
    } = spec;
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .py_1()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(
                    div()
                        .font_family(theme::mono_font())
                        .text_sm()
                        .whitespace_nowrap()
                        .when(failed, |d| d.text_color(theme::error_red()))
                        .child(title),
                )
                .child(div().text_xs().text_color(muted).child(meta)),
        )
        .child(
            Button::new(("library-insert", ix))
                .label("挿入")
                .small()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(tab) = tab {
                        let sql = sql.clone();
                        this.model
                            .update(cx, |m, cx| m.insert_into_editor(tab, &sql, window, cx));
                    }
                })),
        )
        .when_some(snippet_id, |d, id| {
            d.child(
                Button::new(("library-delete", ix))
                    .label("削除")
                    .small()
                    .danger()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, _| m.delete_snippet(id));
                    })),
            )
        })
        .into_any_element()
}
