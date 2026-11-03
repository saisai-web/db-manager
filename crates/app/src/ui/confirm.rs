//! 実行前の確認。WHERE なしの UPDATE / DELETE、DROP / TRUNCATE は、対象を示して確認する。

use super::prelude::*;
use sqltool_core::ConfirmKind;

use super::theme;
use super::workspace::Workspace;

fn describe(kind: ConfirmKind) -> &'static str {
    match kind {
        ConfirmKind::UnfilteredUpdate => "WHERE がない UPDATE です。すべての行が更新されます",
        ConfirmKind::UnfilteredDelete => "WHERE がない DELETE です。すべての行が削除されます",
        ConfirmKind::Drop => "DROP です。対象が削除されます",
        ConfirmKind::Truncate => "TRUNCATE です。すべての行が削除されます",
    }
}

/// 確認を待っているタブがあれば、画面を覆う確認パネルを返す。
pub fn confirm_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme = cx.theme().clone();
    let tab = ws.model.read(cx).active_tab()?;
    let pending = tab.pending_confirm.clone()?;
    let tab_id = tab.id;
    let items: Vec<_> = pending
        .items
        .iter()
        .map(|i| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(theme.muted)
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::warn_amber())
                        .child(describe(i.kind)),
                )
                .child(
                    div()
                        .font_family(theme::mono_font())
                        .text_sm()
                        .child(i.summary.clone()),
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
                    .w(px(560.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("実行前の確認"),
                    )
                    .children(items)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("confirm-no")
                                    .label("やめる")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.model.update(cx, |m, cx| {
                                            m.answer_confirmation(tab_id, false, cx)
                                        });
                                    })),
                            )
                            .child(
                                Button::new("confirm-yes")
                                    .label("実行する")
                                    .danger()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.model.update(cx, |m, cx| {
                                            m.answer_confirmation(tab_id, true, cx)
                                        });
                                    })),
                            ),
                    ),
            ),
    )
}
