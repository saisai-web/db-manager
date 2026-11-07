//! テーブル編集の操作。変更は `EditBuffer` に溜め、保存のときに core へ渡す。

use gpui_kit::component::input::InputContent;
use gpui_kit::component::table::TableEvent;
use gpui_kit::{Context, Window};
use sqltool_core::{
    Access, Command, EditPreviewItem, EditRejection, Event, PlanId, ResultId, TabId, TableRef,
    Value,
};

use super::AppModel;
use super::status::StatusLine;
use crate::ui::{ResultView, RowRef, row_ref};

/// 保存前の確認(プレビュー)。
pub struct EditPreviewState {
    pub plan: PlanId,
    pub items: Vec<EditPreviewItem>,
}

/// 結果ストア上の 1 行の、元の値を読む。退避済みで読み込み中なら `None`。
fn original(view: &ResultView, row: u64, col: usize) -> Option<Value> {
    match view.handle.with_source_row(row, |r| {
        r.cell(col).map(|b| String::from_utf8_lossy(b).into_owned())
    }) {
        Access::Ready(v) => Some(v),
        _ => None,
    }
}

impl AppModel {
    fn view_mut(&mut self, tab: TabId, result: ResultId) -> Option<&mut ResultView> {
        self.tab_mut(tab)?.result_mut(result)
    }

    /// テーブルを開く。`limit` が `None` なら全行。
    pub fn open_table(&mut self, tab: TabId, table: TableRef, limit: Option<u64>) {
        if let Some(t) = self.tab_mut(tab) {
            t.table_limit = limit;
        }
        self.core.send(Command::OpenTable { tab, table, limit });
    }

    /// グリッドのセルを選んだ。編集バーの入力欄に、そのセルの値を入れる。
    pub(super) fn on_table_event(
        &mut self,
        tab: TabId,
        result: ResultId,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let TableEvent::SelectCell(row, col) = event else {
            return;
        };
        let Some(view) = self.view_mut(tab, result) else {
            return;
        };
        let Some(edit) = view.edit.as_mut() else {
            return;
        };
        edit.cell = Some((*row, *col));
        let stored = view.handle.row_count();
        let text = match row_ref(*row, stored, |p| view.handle.source_row(p)) {
            Some(RowRef::Stored(src)) => {
                let buf = view.edit.as_ref().unwrap().buffer.borrow();
                match buf.edited(src, *col) {
                    Some(v) => v.clone(),
                    None => original(view, src, *col).flatten(),
                }
            }
            Some(RowRef::Inserted(ix)) => view
                .edit
                .as_ref()
                .unwrap()
                .buffer
                .borrow()
                .insert_cell(ix, *col)
                .and_then(|c| c.clone().flatten()),
            None => None,
        };
        let input = view.edit.as_ref().unwrap().input.clone();
        input.update(cx, |i, cx| {
            i.set_value(InputContent::from(text.unwrap_or_default()), window, cx)
        });
        cx.notify();
    }

    /// 編集バーの値を、選択中のセルに適用する。`null` なら NULL にする。
    pub fn apply_cell_edit(&mut self, tab: TabId, null: bool, cx: &mut Context<Self>) {
        let Some(result) = self
            .tab(tab)
            .and_then(|t| t.active_result())
            .map(|r| r.handle.id())
        else {
            return;
        };
        let Some(view) = self.view_mut(tab, result) else {
            return;
        };
        let Some(edit) = view.edit.as_ref() else {
            return;
        };
        let Some((row, col)) = edit.cell else { return };
        let value: Value = if null {
            None
        } else {
            Some(edit.input.read(cx).value().to_string())
        };
        match row_ref(row, view.handle.row_count(), |p| view.handle.source_row(p)) {
            Some(RowRef::Stored(src)) => {
                let Some(orig) = original(view, src, col) else {
                    self.set_status(
                        tab,
                        StatusLine::warn("行の読み込み中です。少し待ってからもう一度どうぞ"),
                    );
                    return;
                };
                edit.buffer.borrow_mut().set_cell(src, col, value, &orig);
            }
            Some(RowRef::Inserted(ix)) => {
                edit.buffer
                    .borrow_mut()
                    .set_insert_cell(ix, col, Some(value))
            }
            None => {}
        }
        view.refresh(cx);
        cx.notify();
    }

    pub fn add_row(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(view) = self
            .tab_mut(tab)
            .and_then(|t| t.results.get_mut(t.active_result))
        else {
            return;
        };
        let Some(edit) = view.edit.as_ref() else {
            return;
        };
        edit.buffer
            .borrow_mut()
            .add_insert(view.handle.columns().len());
        view.refresh(cx);
        cx.notify();
    }

    /// 選択中の行を、削除する(もう一度で元に戻す)。追加する行なら、取りやめる。
    pub fn toggle_delete_row(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(view) = self
            .tab_mut(tab)
            .and_then(|t| t.results.get_mut(t.active_result))
        else {
            return;
        };
        let Some(edit) = view.edit.as_ref() else {
            return;
        };
        let Some((row, _)) = edit.cell else { return };
        let mut buf = edit.buffer.borrow_mut();
        match row_ref(row, view.handle.row_count(), |p| view.handle.source_row(p)) {
            Some(RowRef::Stored(src)) if buf.is_deleted(src) => buf.restore_row(src),
            Some(RowRef::Stored(src)) => buf.delete_row(src),
            Some(RowRef::Inserted(ix)) => buf.remove_insert(ix),
            None => {}
        }
        drop(buf);
        view.refresh(cx);
        cx.notify();
    }

    pub fn discard_edits(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(view) = self
            .tab_mut(tab)
            .and_then(|t| t.results.get_mut(t.active_result))
        else {
            return;
        };
        if let Some(edit) = view.edit.as_mut() {
            edit.buffer.borrow_mut().clear();
            edit.cell = None;
        }
        view.refresh(cx);
        cx.notify();
    }

    /// 溜めた変更から、保存する SQL のプレビューを求める。DB にはまだ何も送らない。
    pub fn save_edits(&mut self, tab: TabId) {
        let Some(t) = self.tab(tab) else { return };
        let Some(view) = t.active_result() else {
            return;
        };
        let Some(edit) = view.edit.as_ref() else {
            return;
        };
        let names: Vec<String> = view
            .handle
            .columns()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let incomplete = std::cell::Cell::new(false);
        let changes = edit
            .buffer
            .borrow()
            .to_changes(&edit.source, &names, |row, col| {
                original(view, row, col).unwrap_or_else(|| {
                    incomplete.set(true);
                    None
                })
            });
        let result = view.handle.id();
        if incomplete.get() {
            self.set_status(
                tab,
                StatusLine::warn("行の読み込み中です。少し待ってから保存してください"),
            );
            return;
        }
        self.core.send(Command::PrepareEdit {
            tab,
            result,
            changes,
        });
    }

    pub fn commit_preview(&mut self, tab: TabId) {
        if let Some(p) = self.tab_mut(tab).and_then(|t| t.edit_preview.take()) {
            self.core.send(Command::CommitEdit { tab, plan: p.plan });
        }
    }

    pub fn cancel_preview(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(p) = self.tab_mut(tab).and_then(|t| t.edit_preview.take()) {
            self.core.send(Command::DiscardEdit { plan: p.plan });
        }
        cx.notify();
    }

    pub(super) fn apply_edit_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::EditPreview {
                tab,
                plan,
                statements,
            } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.edit_preview = Some(EditPreviewState {
                        plan,
                        items: statements,
                    });
                }
            }
            Event::EditCommitted {
                tab, statements, ..
            } => {
                // 保存できた。最新の内容を見せるため、同じテーブルを開き直す
                let table = self
                    .tab(tab)
                    .and_then(|t| t.active_result())
                    .and_then(|r| r.edit.as_ref().map(|e| e.source.table.clone()));
                self.set_status(
                    tab,
                    StatusLine::ok(format!("{statements} 件の変更を保存しました")),
                );
                if let Some(table) = table {
                    let limit = self.tab(tab).and_then(|t| t.table_limit);
                    self.open_table(tab, table, limit);
                }
            }
            Event::EditRejected { tab, reason, .. } => {
                let text = match reason {
                    EditRejection::RowCount { actual, sql, .. } => format!(
                        "保存できませんでした(すべて取り消しました)。他の人が先に更新・削除した可能性があります(影響行数 {actual}): {sql}"
                    ),
                    EditRejection::Database { error, .. } => {
                        format!("保存できませんでした(すべて取り消しました): {error}")
                    }
                };
                self.set_status(tab, StatusLine::error(text));
            }
            _ => {}
        }
        cx.notify();
    }
}
