//! SQL エディタ。Tree-sitter による色分けと、行番号つき。

use super::prelude::*;
use gpui_kit::component::input::Editor;

use super::theme;
use super::workspace::Workspace;

pub fn editor_pane(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let theme_colors = cx.theme().clone();
    let Some(tab) = ws.model.read(cx).active_tab() else {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme_colors.muted_foreground)
            .child("「接続…」から接続先を選んでください")
            .into_any_element();
    };
    div()
        .size_full()
        .font_family(theme::mono_font())
        .text_size(px(13.))
        .child(Editor::new(&tab.editor).h_full().appearance(false))
        .into_any_element()
}
