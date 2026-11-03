//! 結果タブ 1 つぶん(グリッドと、編集中の状態)。

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::component::table::TableState;
use sqltool_core::{Core, EditAccess, EditBuffer, ResultHandle, StatementOutcome};

use super::edit_view::EditView;
use super::grid::GridDelegate;
use super::prelude::*;

pub struct ResultView {
    pub handle: ResultHandle,
    pub table: Entity<TableState<GridDelegate>>,
    /// 受信中の行数(進捗イベントで更新)
    pub rows: u64,
    /// 取得上限で止まっている
    pub paused: bool,
    pub outcome: Option<StatementOutcome>,
    /// テーブルを開いた結果で、キーがあれば編集できる
    pub edit: Option<EditView>,
}

impl ResultView {
    pub fn new(core: &Core, handle: ResultHandle, window: &mut Window, cx: &mut App) -> Self {
        let edit = match handle.edit_access() {
            EditAccess::Editable(source) => Some(EditView {
                buffer: Rc::new(RefCell::new(EditBuffer::new())),
                source: source.clone(),
                cell: None,
                input: cx.new(|cx| {
                    InputState::new(window, cx).placeholder("セルを選ぶと、ここで値を編集できます")
                }),
            }),
            _ => None,
        };
        let delegate = GridDelegate::new(
            core.clone(),
            handle.clone(),
            edit.as_ref().map(|e| e.buffer.clone()),
        );
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_resizable(true)
                .sortable(true)
                .cell_selectable(true)
        });
        super::grid::watch_reloads(&handle, &table, cx);
        Self {
            rows: handle.row_count(),
            handle,
            table,
            paused: false,
            outcome: None,
            edit,
        }
    }

    /// 行数や並び順が変わったので、描画を更新する。
    pub fn refresh(&self, cx: &mut App) {
        self.table.update(cx, |t, cx| t.refresh(cx));
    }
}
