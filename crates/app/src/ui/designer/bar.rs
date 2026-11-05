//! ER 設計の上のツールバー。ファイル、元に戻す、表・関係の操作、表示。文字を使わず、アイコンと吹き出しで示す。

use gpui_kit::assets::IconName;
use gpui_kit::component::Selectable as _;
use sqltool_core::design::diagram::Labels;

use super::Designer;
use crate::ui::prelude::*;

/// アイコンだけのボタン(名前は、カーソルを乗せると出る)。
fn tool(id: &'static str, icon: IconName, tip: &'static str) -> Button {
    Button::new(id).icon(icon).small().ghost().tooltip(tip)
}

/// 文字のボタン(物理名・論理名の切り替えなど)。
fn text_tool(id: &'static str, label: &str, active: bool) -> Button {
    Button::new(id)
        .label(label.to_string())
        .small()
        .ghost()
        .selected(active)
}

fn separator(cx: &Context<Designer>) -> impl IntoElement + use<> {
    div().w(px(1.)).h(px(16.)).mx_1().bg(cx.theme().border)
}

pub(super) fn toolbar(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let file = d
        .file
        .as_ref()
        .and_then(|f| f.file_name())
        .map_or("(未保存)".to_string(), |n| {
            n.to_string_lossy().into_owned()
        });
    let title = format!("{file}{}", if d.dirty { " ●" } else { "" });
    let zoom = format!("{:.0}%", d.zoom * 100.0);
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap_1()
        .min_h(px(40.))
        .px_3()
        .py_1()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .mr_2()
                .child("ER 設計"),
        )
        .child(
            tool("d-new", IconName::FilePlus, "新規")
                .on_click(cx.listener(|this, _, w, cx| this.new_design(w, cx))),
        )
        .child(
            tool("d-open", IconName::FolderOpen, "開く…")
                .on_click(cx.listener(|this, _, w, cx| this.open_dialog(w, cx))),
        )
        .child(
            tool("d-save", IconName::Save, "保存")
                .on_click(cx.listener(|this, _, w, cx| this.save(w, cx))),
        )
        .child(
            tool("d-saveas", IconName::SaveAll, "別名で保存…")
                .on_click(cx.listener(|this, _, w, cx| this.save_as(w, cx))),
        )
        .child(separator(cx))
        .child(
            tool("d-undo", IconName::Undo2, "元に戻す")
                .disabled(d.undo.is_empty())
                .on_click(cx.listener(|this, _, w, cx| this.undo(w, cx))),
        )
        .child(
            tool("d-redo", IconName::Redo2, "やり直す")
                .disabled(d.redo.is_empty())
                .on_click(cx.listener(|this, _, w, cx| this.redo(w, cx))),
        )
        .child(separator(cx))
        .child(
            tool("d-add", IconName::Table2, "表を追加")
                .on_click(cx.listener(|this, _, w, cx| this.add_table(w, cx))),
        )
        .child(
            tool("d-connect", IconName::Link, "関係線を引く")
                .selected(d.connect)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.connect = !this.connect;
                    this.pick = None;
                    let hint = if this.connect {
                        "関係線: つなぐ表(または列)をクリックし、続けて参照される側をクリックします"
                    } else {
                        "関係線の操作を終えました"
                    };
                    this.note(false, hint);
                    cx.notify();
                })),
        )
        .child(
            tool("d-delete", IconName::Trash, "選んだ表を削除")
                .disabled(d.selected.is_none())
                .on_click(cx.listener(|this, _, w, cx| this.delete_selected(w, cx))),
        )
        .child(
            tool("d-layout", IconName::LayoutGrid, "自動で配置し直す")
                .on_click(cx.listener(|this, _, w, cx| this.auto_layout(w, cx))),
        )
        .child(separator(cx))
        .child(
            text_tool("d-physical", "物理名", d.labels == Labels::Physical)
                .on_click(cx.listener(|this, _, _, cx| this.set_labels(Labels::Physical, cx))),
        )
        .child(
            text_tool("d-logical", "論理名", d.labels == Labels::Logical)
                .on_click(cx.listener(|this, _, _, cx| this.set_labels(Labels::Logical, cx))),
        )
        .child(separator(cx))
        .child(
            tool("d-zoom-out", IconName::ZoomOut, "縮小")
                .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom / 1.25, cx))),
        )
        .child(
            text_tool("d-zoom-reset", &zoom, false)
                .tooltip("100% に戻す")
                .on_click(cx.listener(|this, _, _, cx| this.set_zoom(1.0, cx))),
        )
        .child(
            tool("d-zoom-in", IconName::ZoomIn, "拡大")
                .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom * 1.25, cx))),
        )
        .child(
            tool("d-zoom-fit", IconName::Maximize, "全体を表示").on_click(cx.listener(
                |this, _, w, cx| {
                    this.fit(w, 2.0);
                    cx.notify();
                },
            )),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .mr_2()
                .child(title),
        )
        .child(
            Button::new("d-export")
                .icon(IconName::SquareTerminal)
                .label("SQL・書き出し")
                .small()
                .primary()
                .on_click(
                    cx.listener(|this, _, w, cx| this.open_export(super::SidePanel::Sql, w, cx)),
                ),
        )
        .child(
            tool("d-close", IconName::Close, "閉じる").on_click(cx.listener(|this, _, _, cx| {
                this.visible = false;
                cx.notify();
                this.model.update(cx, |_, cx| cx.notify());
            })),
        )
}
