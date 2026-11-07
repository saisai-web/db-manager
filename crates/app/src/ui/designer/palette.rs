//! 左端のレール。上に作図の道具(表・関係線・自動配置。削除は、表と列の「×」)を大きく並べ、
//! 区切り線の下に、テキスト記法の開閉を置く。

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

use super::Designer;
use crate::ui::prelude::*;

const RAIL_WIDTH: f32 = 52.;

impl Designer {
    /// 関係線を引くモードの入り切り。
    fn toggle_connect(&mut self, cx: &mut Context<Self>) {
        self.connect = !self.connect;
        self.pick = None;
        let hint = if self.connect {
            "関係線: つなぐ表(または列)をクリックし、続けて参照される側をクリックします"
        } else {
            "関係線の操作を終えました"
        };
        self.note(false, hint);
        cx.notify();
    }
}

/// レールの 1 つ分: 大きなアイコンと、その下の短い名前。
fn item(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &Context<Designer>,
) -> impl IntoElement {
    let theme = cx.theme();
    let ink = if on { theme.primary } else { theme.foreground };
    div()
        .id(id)
        .w(px(RAIL_WIDTH - 8.))
        .py_1p5()
        .flex()
        .flex_col()
        .items_center()
        .gap_0p5()
        .rounded_md()
        .when(on, |b| b.bg(theme.accent))
        .cursor_pointer()
        .hover(|h| h.bg(theme.accent))
        .child(Icon::new(icon).size(px(18.)).text_color(ink))
        .child(div().text_size(px(9.)).text_color(ink).child(label))
        .on_click(on_click)
        .test_support()
}

pub(super) fn rail(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let add = cx.listener(|this, _, w, cx| this.add_table(w, cx));
    let connect = cx.listener(|this, _, _, cx| this.toggle_connect(cx));
    let layout = cx.listener(|this, _, w, cx| this.auto_layout(w, cx));
    let text = cx.listener(|this, _, w, cx| this.set_show_text(!this.show_text, w, cx));
    let tools = div()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .child(item("d-add", IconName::Table2, "表を追加", false, add, cx))
        .child(item(
            "d-connect",
            IconName::Link,
            "関係線",
            d.connect,
            connect,
            cx,
        ))
        .child(item(
            "d-layout",
            IconName::LayoutGrid,
            "自動配置",
            false,
            layout,
            cx,
        ));
    div()
        .w(px(RAIL_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .py_2()
        .gap_2()
        .border_r_1()
        .border_color(border)
        .child(tools)
        // 作図の道具と、テキストの間の線
        .child(div().w(px(RAIL_WIDTH - 16.)).h(px(1.)).bg(border))
        .child(item(
            "d-text",
            IconName::PanelLeft,
            "テキスト",
            d.show_text,
            text,
            cx,
        ))
}
