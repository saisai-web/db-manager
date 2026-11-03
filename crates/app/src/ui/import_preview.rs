//! CSV の取り込み前の確認。文字コード、区切り文字、先頭の行、列の対応づけを見せる。

use sqltool_core::TabId;

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;
use crate::model::default_mapping;

const PREVIEW_ROWS: usize = 5;

fn encoding_label(e: sqltool_core::TextEncoding) -> &'static str {
    match e {
        sqltool_core::TextEncoding::Utf8 => "UTF-8",
        sqltool_core::TextEncoding::Utf8Bom => "UTF-8(BOM 付き)",
        sqltool_core::TextEncoding::ShiftJis => "Shift_JIS",
    }
}

fn delimiter_label(d: u8) -> String {
    match d {
        b'\t' => "タブ".into(),
        other => format!("「{}」", other as char),
    }
}

pub fn import_preview_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    let model = ws.model.read(cx);
    let tab = model.active_tab()?;
    let p = tab.import_preview.as_ref()?;
    let tab_id: TabId = tab.id;
    let table_columns = model.import_table_columns(tab_id);
    let mapping = table_columns
        .as_ref()
        .map(|c| default_mapping(&p.headers, p.has_header, c));
    let ready = mapping.as_ref().is_some_and(|m| !m.is_empty()) && !p.headers.is_empty();
    let summary = match &mapping {
        None => "表の列を読み込み中…".to_string(),
        Some(m) if m.is_empty() => {
            "対応する列がありません。見出しの有無を切り替えるか、列名を合わせてください".to_string()
        }
        Some(m) => format!(
            "対応: {}",
            m.iter()
                .map(|(i, c)| format!("{} → {c}", p.headers.get(*i).map_or("?", String::as_str)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let header_row = div()
        .flex()
        .flex_row()
        .gap_3()
        .children(p.headers.iter().map(|h| {
            div()
                .w(px(110.))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .overflow_hidden()
                .child(h.clone())
        }));
    let rows: Vec<_> = p
        .rows
        .iter()
        .take(PREVIEW_ROWS)
        .map(|r| {
            div().flex().flex_row().gap_3().children(r.iter().map(|c| {
                let cell = div()
                    .w(px(110.))
                    .text_xs()
                    .overflow_hidden()
                    .whitespace_nowrap();
                match c {
                    Some(v) => cell.child(v.clone()),
                    None => cell
                        .italic()
                        .text_color(theme_colors.muted_foreground)
                        .child("NULL"),
                }
            }))
        })
        .collect();
    let has_header = p.has_header;
    let meta = format!(
        "{} / 文字コード {} / 区切り {}",
        p.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        encoding_label(p.encoding),
        delimiter_label(p.delimiter)
    );
    let title = format!("「{}」に取り込む", p.table.name);
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
                    .w(px(720.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme_colors.border)
                    .bg(theme_colors.background)
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_xs().text_color(theme_colors.muted_foreground).child(meta))
                    .child(
                        div().flex().flex_row().items_center().gap_2().child(
                            Button::new("import-header")
                                .label(if has_header { "1 行目は見出し: ON" } else { "1 行目は見出し: OFF" })
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.update(cx, |m, cx| m.set_import_header(tab_id, !has_header, cx));
                                })),
                        ),
                    )
                    .child(div().flex().flex_col().gap_1().p_2().rounded_md().bg(theme_colors.muted).child(header_row).children(rows))
                    .child(div().text_xs().text_color(if ready { theme::ok_green() } else { theme::warn_amber() }).child(summary))
                    .child(
                        div().text_xs().text_color(theme_colors.muted_foreground).child(
                            "1 つのトランザクションで取り込みます。1 行でも失敗すれば、すべて取り消します(空のフィールドは NULL になります)。",
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(Button::new("import-cancel").label("キャンセル").on_click(cx.listener(move |this, _, _, cx| {
                                this.model.update(cx, |m, cx| m.cancel_import(tab_id, cx));
                            })))
                            .child(Button::new("import-run").label("取り込む").primary().disabled(!ready).on_click(cx.listener(
                                move |this, _, _, cx| this.model.update(cx, |m, _| m.confirm_import(tab_id)),
                            ))),
                    ),
            ),
    )
}
