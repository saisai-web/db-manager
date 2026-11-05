//! 表の詳細エディタの画面。

use gpui_kit::component::input::Input;
use sqltool_core::design::ForeignKey;

use super::Designer;
use super::detail::Flag;
use super::dialog::modal;
use crate::ui::prelude::*;

fn labeled(
    label: &'static str,
    width: f32,
    body: impl IntoElement,
    muted: Hsla,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w(px(width))
        .child(div().text_xs().text_color(muted).w(px(44.)).child(label))
        .child(div().flex_1().child(body))
}

fn flag_button(
    id: (&'static str, usize),
    label: &'static str,
    on: bool,
    flag: Flag,
    col: usize,
    cx: &mut Context<Designer>,
) -> impl IntoElement + use<> {
    let b = Button::new(id).label(label).small();
    let b = if on { b.primary() } else { b.ghost() };
    div()
        .w(px(40.))
        .child(b.on_click(cx.listener(move |this, _, w, cx| this.toggle_flag(col, flag, w, cx))))
}

fn fk_text(fk: &ForeignKey, d: &Designer) -> String {
    let refs = d.design.ref_columns(fk);
    format!(
        "{} → {}.{}   削除時: {}  更新時: {}",
        fk.columns.join(", "),
        fk.ref_table,
        refs.join(", "),
        fk.on_delete.label(),
        fk.on_update.label()
    )
}

pub(super) fn detail_dialog(
    d: &Designer,
    window: &Window,
    cx: &mut Context<Designer>,
) -> Option<AnyElement> {
    let detail = d.detail.as_ref()?;
    let t = d.design.tables.get(detail.table)?;
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;

    let head = |w: f32, text: &'static str| {
        div()
            .w(px(w))
            .text_xs()
            .text_color(muted)
            .whitespace_nowrap()
            .child(text)
    };
    let header_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_3()
        .py_1()
        .child(head(54., "順"))
        .child(head(150., "列名"))
        .child(head(130., "論理名"))
        .child(head(130., "型"))
        .child(head(110., "既定値"))
        .child(div().flex_1().text_xs().text_color(muted).child("備考"))
        .child(head(40., "PK"))
        .child(head(40., "非NULL"))
        .child(head(40., "一意"))
        .child(head(30., ""));

    let table = detail.table;
    let last = t.columns.len().saturating_sub(1);
    let rows: Vec<_> = t
        .columns
        .iter()
        .zip(&detail.cols)
        .enumerate()
        .map(|(j, (c, inputs))| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_3()
                .py(px(2.))
                .child(
                    div()
                        .w(px(54.))
                        .flex()
                        .flex_row()
                        .gap_1()
                        .child(
                            Button::new(("d-up", j))
                                .label("↑")
                                .ghost()
                                .small()
                                .disabled(j == 0)
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.detail_move_column(j, -1, w, cx)
                                })),
                        )
                        .child(
                            Button::new(("d-down", j))
                                .label("↓")
                                .ghost()
                                .small()
                                .disabled(j == last)
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.detail_move_column(j, 1, w, cx)
                                })),
                        ),
                )
                .child(div().w(px(150.)).child(Input::new(&inputs.name).small()))
                .child(div().w(px(130.)).child(Input::new(&inputs.logical).small()))
                .child(
                    div()
                        .w(px(130.))
                        .flex()
                        .flex_row()
                        .gap_1()
                        .child(div().flex_1().child(Input::new(&inputs.ty).small()))
                        .child(
                            Button::new(("d-ty", j))
                                .label("▾")
                                .small()
                                .ghost()
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.open_type_menu(table, j, true, w, cx)
                                })),
                        ),
                )
                .child(div().w(px(110.)).child(Input::new(&inputs.default).small()))
                .child(div().flex_1().child(Input::new(&inputs.comment).small()))
                .child(flag_button(
                    ("d-pk", j),
                    "PK",
                    c.primary,
                    Flag::Primary,
                    j,
                    cx,
                ))
                .child(flag_button(
                    ("d-nn", j),
                    "NN",
                    !c.nullable,
                    Flag::NotNull,
                    j,
                    cx,
                ))
                .child(flag_button(
                    ("d-uq", j),
                    "UQ",
                    c.unique,
                    Flag::Unique,
                    j,
                    cx,
                ))
                .child(
                    div().w(px(30.)).child(
                        Button::new(("d-del", j))
                            .label("×")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.detail_remove_column(j, w, cx)
                            })),
                    ),
                )
        })
        .collect();

    let relations: Vec<_> = t
        .foreign_keys
        .iter()
        .map(|fk| {
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("関係: {}", fk_text(fk, d)))
        })
        .collect();

    let scale = format!("{:.0}%", t.factor() * 100.0);
    let body = div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_3()
                .p_3()
                .border_b_1()
                .border_color(border)
                .child(labeled(
                    "物理名",
                    300.,
                    Input::new(&detail.name).small(),
                    muted,
                ))
                .child(labeled(
                    "論理名",
                    300.,
                    Input::new(&detail.logical).small(),
                    muted,
                ))
                .child(labeled(
                    "備考",
                    300.,
                    Input::new(&detail.comment).small(),
                    muted,
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(div().text_xs().text_color(muted).mr_1().child("表の大きさ"))
                        .child(
                            Button::new("d-smaller")
                                .label("−")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.detail_scale(Some(-0.1), w, cx)
                                })),
                        )
                        .child(div().w(px(44.)).text_center().text_sm().child(scale))
                        .child(
                            Button::new("d-bigger")
                                .label("＋")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.detail_scale(Some(0.1), w, cx)
                                })),
                        )
                        .child(
                            Button::new("d-reset-size")
                                .label("等倍")
                                .small()
                                .ghost()
                                .on_click(
                                    cx.listener(|this, _, w, cx| this.detail_scale(None, w, cx)),
                                ),
                        ),
                ),
        )
        .child(header_row)
        .child(
            div()
                .id("detail-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows),
        )
        .when(!relations.is_empty(), |b| {
            b.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .children(relations),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .p_3()
                .border_t_1()
                .border_color(border)
                .child(
                    Button::new("d-add-col")
                        .label("＋ 列を追加")
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, w, cx| this.detail_add_column(w, cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("d-del-table")
                        .label("この表を削除")
                        .small()
                        .on_click(cx.listener(|this, _, w, cx| this.detail_delete_table(w, cx))),
                ),
        );
    Some(modal(
        "detail-dialog",
        format!("表の詳細: {}", t.name),
        (1080.0, None),
        window,
        body,
        |this, _, cx| this.close_detail(cx),
        cx,
    ))
}
