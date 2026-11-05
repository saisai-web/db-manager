//! 図の表 1 つぶんの箱。見出し(ドラッグで移動・名前をクリックで編集・「＋」で列を足す・「詳細」)と、
//! 列の行(名前・型をクリックで編集)、右下の大きさ変更の角。

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputState};
use sqltool_core::design::diagram::{
    BADGE_W, HEADER_H, PAD, ROW_H, column_label, header_label, row_height,
};

use super::Designer;
use super::diagram::MARGIN;
use super::inline::Field;
use crate::ui::prelude::*;

/// 見出しの小さなアイコンボタン(列を足す・詳細)。押しても、表の移動は始まらない。
fn mini_button(
    id: (&'static str, usize),
    icon: IconName,
    k: f32,
    (border, background, accent): (Hsla, Hsla, Hsla),
    on_click: impl Fn(&mut Designer, &mut Window, &mut Context<Designer>) + 'static,
    cx: &mut Context<Designer>,
) -> AnyElement {
    div()
        .id(id)
        .w(px(22. * k))
        .h(px((HEADER_H - 8.) * k))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4. * k))
        .border_1()
        .border_color(border)
        .bg(background)
        .cursor_pointer()
        .hover(|h| h.bg(accent))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .on_click(cx.listener(move |this, _, w, cx| {
            cx.stop_propagation();
            on_click(this, w, cx);
        }))
        .child(Icon::new(icon).size(px(12. * k)))
        .test_support()
        .into_any_element()
}

/// その場編集の入力欄。Tab で次の列へ(Shift で前の列へ)、Esc でやめる。
fn editor(
    input: &Entity<InputState>,
    width: Pixels,
    height: Pixels,
    fill: bool,
    cx: &mut Context<Designer>,
) -> AnyElement {
    div()
        .when(fill, |d| d.flex_1())
        .when(!fill, |d| d.w(width))
        .h(height)
        .overflow_hidden()
        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
            let key = ev.keystroke.key.as_str();
            if matches!(key, "tab" | "escape") {
                cx.stop_propagation();
                this.inline_key(key, ev.keystroke.modifiers.shift, w, cx);
            }
        }))
        .child(Input::new(input).xsmall())
        .into_any_element()
}

pub(super) fn table_box(
    d: &Designer,
    ix: usize,
    cx: &mut Context<Designer>,
) -> impl IntoElement + use<> {
    let theme = cx.theme().clone();
    let t = &d.design.tables[ix];
    // 文字・余白・列の高さの基準は図の倍率だけ。表の大きさ(`f`)は、箱の幅と見出し・行の高さに足す余白になる
    let k = d.zoom;
    let f = t.factor();
    let r = d.derived.geo.rects[ix];
    let selected = d.selected.as_deref() == Some(t.name.as_str());
    let picked_table = d.pick.as_ref().is_some_and(|p| p.table == t.name);
    let (main, sub) = header_label(t, d.labels);
    let name = t.name.clone();
    let editing = |column: Option<usize>, field: Field| {
        d.inline
            .as_ref()
            .filter(|i| i.is(ix, column, field))
            .map(|i| i.input.clone())
    };

    let title: AnyElement = match editing(None, Field::Name) {
        Some(input) => editor(&input, px(120. * k), px((HEADER_H - 6.) * k), false, cx),
        None => div()
            .id(("tn", ix))
            .text_size(px(13. * k))
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .cursor_text()
            .on_click(cx.listener(move |this, _, w, cx| {
                // 動かしたあとの「クリック」は、編集ではない
                if !this.drag.as_ref().is_some_and(|d| d.moved) {
                    this.begin_inline(ix, None, Field::Name, w, cx);
                }
            }))
            .child(main)
            .test_support()
            .into_any_element(),
    };

    let header = div()
        .id(("th", ix))
        .h(px(HEADER_H * k))
        .px(px(PAD * k))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6. * k))
        .bg(theme.primary.opacity(
            if selected || (picked_table && d.pick.as_ref().is_some_and(|p| p.column.is_none())) {
                0.35
            } else {
                0.15
            },
        ))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, {
            let name = name.clone();
            cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                cx.stop_propagation();
                if this.connect {
                    this.click(&name, None, w, cx);
                } else if ev.click_count >= 2 {
                    this.select(&name, w, cx);
                    this.open_detail(ix, w, cx);
                } else {
                    this.begin_drag(ix, ev.position, cx);
                    this.select(&name, w, cx);
                }
            })
        })
        .child(title)
        .when(!sub.is_empty(), |h| {
            h.child(
                div()
                    .text_size(px(11. * k))
                    .text_color(theme.muted_foreground)
                    .whitespace_nowrap()
                    .child(sub),
            )
        })
        .child(div().flex_1())
        .child(mini_button(
            ("t-add", ix),
            IconName::Plus,
            k,
            (theme.border, theme.background, theme.accent),
            move |this, w, cx| this.add_column(ix, w, cx),
            cx,
        ))
        .child(mini_button(
            ("t-detail", ix),
            IconName::Settings2,
            k,
            (theme.border, theme.background, theme.accent),
            move |this, w, cx| this.open_detail(ix, w, cx),
            cx,
        ))
        .test_support();

    let rows: Vec<_> =
        t.columns
            .iter()
            .enumerate()
            .map(|(j, c)| {
                let (label, ty) = column_label(c, d.labels);
                let badge = match (c.primary, t.is_foreign(&c.name)) {
                    (true, true) => "PK,FK",
                    (true, false) => "PK",
                    (false, true) => "FK",
                    _ => "",
                };
                let hot = d.pick.as_ref().is_some_and(|p| {
                    p.table == t.name && p.column.as_deref() == Some(c.name.as_str())
                });
                let (table, column) = (t.name.clone(), c.name.clone());
                let cell_h = px((ROW_H - 2.) * k);
                let name_cell: AnyElement = match editing(Some(j), Field::Name) {
                    Some(input) => editor(&input, px(0.), cell_h, true, cx),
                    None => div()
                        .id(("cn", ix * 10_000 + j))
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .cursor_text()
                        .when(c.primary, |x| x.font_weight(FontWeight::SEMIBOLD))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.begin_inline(ix, Some(j), Field::Name, w, cx)
                        }))
                        .child(label)
                        .test_support()
                        .into_any_element(),
                };
                let type_cell: Option<AnyElement> = if ty.is_empty() {
                    None
                } else {
                    Some(match editing(Some(j), Field::Type) {
                        Some(input) => editor(&input, px(96. * k), cell_h, false, cx),
                        None => div()
                            .id(("cty", ix * 10_000 + j))
                            .h_full()
                            .flex()
                            .items_center()
                            .text_size(px(11. * k))
                            .text_color(theme.muted_foreground)
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.open_type_menu(ix, j, false, w, cx)
                            }))
                            .child(format!("{ty} ▾"))
                            .test_support()
                            .into_any_element(),
                    })
                };
                div()
                    .id(("col", ix * 10_000 + j))
                    .h(px(row_height(f) * k))
                    .px(px(6. * k))
                    .flex()
                    .flex_row()
                    .items_center()
                    .text_size(px(12. * k))
                    .when(hot, |x| x.bg(theme.primary.opacity(0.3)))
                    .hover(|h| h.bg(theme.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.click(&table, Some(&column), w, cx)),
                    )
                    .child(
                        div()
                            .w(px((BADGE_W - 6.) * k))
                            .text_size(px(9. * k))
                            .text_color(theme.muted_foreground)
                            .child(badge),
                    )
                    .child(name_cell)
                    .children(type_cell)
                    .test_support()
            })
            .collect();

    let grip = div()
        .id(("grip", ix))
        .absolute()
        .right_0()
        .bottom_0()
        .w(px(16. * k.max(0.7)))
        .h(px(14. * k.max(0.7)))
        .flex()
        .items_end()
        .justify_end()
        .text_color(theme.muted_foreground)
        .cursor_nwse_resize()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                cx.stop_propagation();
                this.commit_inline(w, cx);
                this.begin_resize(ix, ev.position, cx);
            }),
        )
        .child(Icon::new(IconName::ResizeCorner).size(px(10. * k.max(0.7))))
        .test_support();

    div()
        .absolute()
        .left(px((r.x + MARGIN) * d.zoom))
        .top(px((r.y + MARGIN) * d.zoom))
        .w(px(r.w * d.zoom))
        .h(px(r.h * d.zoom))
        .rounded_lg()
        .shadow_sm()
        .border_1()
        .border_color(if selected || picked_table {
            theme.primary
        } else {
            theme.border
        })
        .bg(theme.background)
        .overflow_hidden()
        .child(header)
        .children(rows)
        .child(grip)
}
