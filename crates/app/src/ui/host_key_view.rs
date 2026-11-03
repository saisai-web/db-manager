//! 初めて接続する SSH サーバーのホスト鍵の確認。指紋を見せ、利用者が信頼したときだけ接続を続ける。

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;
use crate::model::Retry;

/// 確認を待っていれば、画面を覆う確認パネルを返す(接続フォームでのテスト中は、フォームの中に出す)。
pub fn host_key_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    let p = ws
        .model
        .read(cx)
        .host_key
        .clone()
        .filter(|p| p.retry != Some(Retry::Test))?;
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
                            .child("SSH サーバーのホスト鍵を確認してください"),
                    )
                    .child(div().text_sm().child(format!(
                        "{}:{} には、初めて接続します。",
                        p.host, p.port
                    )))
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(theme_colors.muted)
                            .font_family(theme::mono_font())
                            .text_sm()
                            .child(p.fingerprint.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::warn_amber())
                            .child("この指紋が、サーバーの管理者に聞いたものと一致するときだけ、信頼してください。一致しないなら、なりすましの可能性があります。"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("host-key-cancel")
                                    .label("キャンセル")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.update(cx, |m, cx| m.dismiss_host_key(cx));
                                    })),
                            )
                            .child(
                                Button::new("host-key-trust")
                                    .label(if p.retry.is_some() {
                                        "信頼して接続"
                                    } else {
                                        "信頼する"
                                    })
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.update(cx, |m, cx| m.trust_host_key(cx));
                                    })),
                            ),
                    ),
            ),
    )
}
