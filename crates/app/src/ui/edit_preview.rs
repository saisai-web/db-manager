//! 保存前の確認。実行する INSERT / UPDATE / DELETE を見せ、承認されたら 1 つのトランザクションで実行する。

use super::edit_bar::kind_label;
use super::prelude::*;
use super::theme;
use super::workspace::Workspace;

pub fn edit_preview_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    let tab = ws.model.read(cx).active_tab()?;
    let preview = tab.edit_preview.as_ref()?;
    let tab_id = tab.id;
    let count = preview.items.len();
    let items: Vec<_> = preview
        .items
        .iter()
        .map(|i| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(theme_colors.muted)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme_colors.muted_foreground)
                        .child(kind_label(i.kind)),
                )
                .child(
                    div()
                        .font_family(theme::mono_font())
                        .text_sm()
                        .child(i.sql.clone()),
                )
        })
        .collect();
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
                    .max_h(px(640.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme_colors.border)
                    .bg(theme_colors.background)
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(format!("{count} 件の変更を保存します")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme_colors.muted_foreground)
                            .child("1 つのトランザクションで実行します。どれかの影響行数が 1 でなければ、すべて取り消します。"),
                    )
                    .child(div().id("preview-scroll").flex().flex_col().gap_2().overflow_y_scroll().children(items))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(Button::new("preview-cancel").label("キャンセル").on_click(cx.listener(
                                move |this, _, _, cx| this.model.update(cx, |m, cx| m.cancel_preview(tab_id, cx)),
                            )))
                            .child(Button::new("preview-commit").label("保存する").primary().on_click(cx.listener(
                                move |this, _, _, cx| this.model.update(cx, |m, _| m.commit_preview(tab_id)),
                            ))),
                    ),
            ),
    )
}
