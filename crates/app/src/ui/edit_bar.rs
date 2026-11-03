//! 結果グリッドの上の編集バー。選択したセルの値を編集し、行の追加・削除、保存・破棄をする。

use gpui_kit::component::input::{Input, InputState};
use sqltool_core::{ChangeKind, TabId};

use super::prelude::*;
use super::workspace::Workspace;

/// 描画に必要な値だけを写したもの(モデルの借用を、リスナーの登録より前に手放すため)。
pub struct EditBarSnapshot {
    tab: TabId,
    input: Entity<InputState>,
    column: Option<String>,
    updates: usize,
    deletes: usize,
    inserts: usize,
    dirty: bool,
    idle: bool,
    read_only: bool,
}

pub fn snapshot(ws: &Workspace, cx: &App) -> Option<EditBarSnapshot> {
    let tab = ws.model.read(cx).active_tab()?;
    let view = tab.active_result()?;
    let edit = view.edit.as_ref()?;
    let (updates, deletes, inserts) = edit.buffer.borrow().counts();
    Some(EditBarSnapshot {
        tab: tab.id,
        input: edit.input.clone(),
        column: edit
            .cell
            .and_then(|(_, c)| view.handle.columns().get(c).map(|c| c.name.clone())),
        updates,
        deletes,
        inserts,
        dirty: edit.buffer.borrow().is_dirty(),
        idle: tab.is_idle(),
        read_only: tab.read_only,
    })
}

pub fn edit_bar(s: EditBarSnapshot, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let theme = cx.theme().clone();
    let tab = s.tab;
    let has_cell = s.column.is_some();
    let summary = format!(
        "変更 {} 行 / 削除 {} 行 / 追加 {} 行",
        s.updates, s.deletes, s.inserts
    );
    let can_save = s.dirty && s.idle && !s.read_only;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .w(px(120.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(s.column.clone().unwrap_or_else(|| "セルを選択".into())),
        )
        .child(
            div()
                .flex_1()
                .child(Input::new(&s.input).disabled(!has_cell)),
        )
        .child(
            Button::new("cell-apply")
                .label("適用")
                .small()
                .primary()
                .disabled(!has_cell)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model
                        .update(cx, |m, cx| m.apply_cell_edit(tab, false, cx));
                })),
        )
        .child(
            Button::new("cell-null")
                .label("NULL にする")
                .small()
                .disabled(!has_cell)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model
                        .update(cx, |m, cx| m.apply_cell_edit(tab, true, cx));
                })),
        )
        .child(
            Button::new("row-add")
                .label("＋ 行を追加")
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, cx| m.add_row(tab, cx));
                })),
        )
        .child(
            Button::new("row-delete")
                .label("行を削除/戻す")
                .small()
                .disabled(!has_cell)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, cx| m.toggle_delete_row(tab, cx));
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(summary),
        )
        .child(
            Button::new("edit-discard")
                .label("破棄")
                .small()
                .disabled(!s.dirty)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, cx| m.discard_edits(tab, cx));
                })),
        )
        .child(
            Button::new("edit-save")
                .label("保存…")
                .small()
                .primary()
                .disabled(!can_save)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.save_edits(tab));
                })),
        )
}

/// プレビューの文の種類の表示名。
pub fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Update => "UPDATE",
        ChangeKind::Insert => "INSERT",
        ChangeKind::Delete => "DELETE",
    }
}
