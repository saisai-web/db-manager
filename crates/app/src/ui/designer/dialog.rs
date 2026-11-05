//! 画面の上に重ねるダイアログ(表の詳細・SQL と書き出し)の枠。背景をクリックすると閉じる。

use super::side::side_panel;
use super::{Designer, SidePanel};
use crate::ui::prelude::*;

/// 画面の中央に出す枠。`body` は、枠の残りの高さいっぱいに広がる。
pub(super) fn modal(
    id: &'static str,
    title: String,
    (width, height): (f32, Option<f32>),
    window: &Window,
    body: impl IntoElement,
    on_close: impl Fn(&mut Designer, &mut Window, &mut Context<Designer>) + Clone + 'static,
    cx: &mut Context<Designer>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let view = window.viewport_size();
    let height = height.unwrap_or((f32::from(view.height) * 0.9).max(320.0));
    let width = width.min(f32::from(view.width) - 24.0);
    let close = on_close.clone();
    div()
        .id(id)
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0., 0., 0., 0.4))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, w, cx| close(this, w, cx)),
        )
        .child(
            div()
                .w(px(width))
                .h(px(height))
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .overflow_hidden()
                // 枠の中のクリックは、背景に届かせない
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .h(px(40.))
                        .px_3()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(
                            Button::new((id, 1usize))
                                .label("閉じる")
                                .small()
                                .on_click(cx.listener(move |this, _, w, cx| on_close(this, w, cx))),
                        ),
                )
                .child(div().flex_1().min_h_0().child(body)),
        )
        .into_any_element()
}

/// CREATE 文・点検・設計書の書き出し。
pub(super) fn export_dialog(
    d: &Designer,
    window: &Window,
    cx: &mut Context<Designer>,
) -> AnyElement {
    modal(
        "export-dialog",
        "SQL・書き出し".into(),
        (900.0, None),
        window,
        side_panel(d, cx),
        |this, _, cx| this.close_export(cx),
        cx,
    )
}

impl Designer {
    pub(super) fn open_export(
        &mut self,
        panel: SidePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_inline(window, cx);
        self.panel = panel;
        self.export_open = true;
        self.refresh(); // CREATE 文は、開いているあいだだけ作る
        cx.notify();
    }

    pub(super) fn close_export(&mut self, cx: &mut Context<Self>) {
        self.export_open = false;
        self.refresh();
        cx.notify();
    }
}
