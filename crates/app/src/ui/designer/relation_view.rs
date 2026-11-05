//! 関係の詳細ダイアログの画面。

use gpui_kit::component::input::Input;
use sqltool_core::design::RefAction;

use super::Designer;
use super::dialog::modal;
use super::relation::Toggle;
use crate::ui::prelude::*;

const ACTIONS: [RefAction; 5] = [
    RefAction::NoAction,
    RefAction::Restrict,
    RefAction::Cascade,
    RefAction::SetNull,
    RefAction::SetDefault,
];

/// 関係の多重度の説明(親側と子側の多重度)。
fn cardinality(parent_required: bool, one_to_one: bool) -> (&'static str, &'static str) {
    (
        if parent_required { "1" } else { "0..1" },
        if one_to_one { "0..1" } else { "0..*" },
    )
}

fn toggle_button(
    id: &'static str,
    label: &'static str,
    on: bool,
    toggle: Toggle,
    cx: &mut Context<Designer>,
) -> impl IntoElement + use<> {
    let b = Button::new(id).label(label).small();
    let b = if on { b.primary() } else { b.ghost() };
    b.on_click(cx.listener(move |this, _, w, cx| this.relation_toggle(toggle, w, cx)))
}

fn action_row(
    title: &'static str,
    id: &'static str,
    on_delete: bool,
    current: RefAction,
    muted: Hsla,
    cx: &mut Context<Designer>,
) -> impl IntoElement + use<> {
    let chips: Vec<_> = ACTIONS
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let b = Button::new((id, i)).label(a.label()).small();
            let b = if a == current { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, w, cx| this.relation_action(on_delete, a, w, cx)))
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(title))
        .child(div().flex().flex_row().flex_wrap().gap_1().children(chips))
}

pub(super) fn relation_dialog(
    d: &Designer,
    window: &Window,
    cx: &mut Context<Designer>,
) -> Option<AnyElement> {
    let r = d.relation.as_ref()?;
    let child = d.design.tables.get(r.table)?;
    let fk = child.foreign_keys.get(r.fk)?;
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;

    let refs = d.design.ref_columns(fk);
    let required = child.fk_required(fk);
    let one = child.fk_is_one_to_one(fk);
    let (parent_side, child_side) = cardinality(required, one);
    let auto_name = child.fk_name(fk);

    let body = div()
        .size_full()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(format!(
                    "{}.{}  →  {}.{}",
                    child.name,
                    fk.columns.join(", "),
                    fk.ref_table,
                    refs.join(", ")
                )))
                .child(div().text_xs().text_color(muted).child(format!(
                    "{} [{parent_side}]  ――  [{child_side}] {}",
                    fk.ref_table, child.name
                ))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("制約名(空なら自動: {auto_name})")),
                )
                .child(Input::new(&r.name).small()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(toggle_button(
                    "r-req",
                    "親が必須(NOT NULL)",
                    required,
                    Toggle::Required,
                    cx,
                ))
                .child(toggle_button(
                    "r-one",
                    "1 対 1(一意)",
                    one,
                    Toggle::OneToOne,
                    cx,
                )),
        )
        .child(action_row(
            "親を削除したとき",
            "r-del",
            true,
            fk.on_delete,
            muted,
            cx,
        ))
        .child(action_row(
            "親の値を変えたとき",
            "r-upd",
            false,
            fk.on_update,
            muted,
            cx,
        ))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .flex_row()
                .pt_2()
                .border_t_1()
                .border_color(border)
                .child(
                    Button::new("r-drop")
                        .label("この関係を消す")
                        .small()
                        .on_click(cx.listener(|this, _, w, cx| this.relation_remove(w, cx))),
                ),
        );
    Some(modal(
        "relation-dialog",
        format!("関係の詳細: {} → {}", child.name, fk.ref_table),
        (560.0, Some(440.0)),
        window,
        body,
        |this, _, cx| this.close_relation(cx),
        cx,
    ))
}
