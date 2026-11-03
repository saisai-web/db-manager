//! 編集中のテーブルの画面状態。変更は DB に送らず、`EditBuffer` に溜める。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::Entity;
use gpui_kit::component::input::InputState;
use sqltool_core::{EditBuffer, TableSource};

pub struct EditView {
    /// グリッドの描画と共有する(同じスレッドの中でだけ使う)
    pub buffer: Rc<RefCell<EditBuffer>>,
    pub source: Arc<TableSource>,
    /// 選択中のセル(表示上の行位置、列)
    pub cell: Option<(usize, usize)>,
    /// 選択したセルの値を編集する入力欄
    pub input: Entity<InputState>,
}

/// グリッドの 1 行が、何を指すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    /// 結果ストアにある行(並べ替え前の行番号)
    Stored(u64),
    /// 追加しようとしている行(0 始まり)
    Inserted(usize),
}

/// 表示上の行位置を、行の指す先にする。受信済みの行の後ろに、追加する行が続く。
pub fn row_ref(
    position: usize,
    stored_rows: u64,
    source_row: impl Fn(u64) -> Option<u64>,
) -> Option<RowRef> {
    let stored = stored_rows as usize;
    if position < stored {
        source_row(position as u64).map(RowRef::Stored)
    } else {
        Some(RowRef::Inserted(position - stored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_rows_follow_the_sort_order_and_inserted_rows_come_last() {
        // 並べ替えで、表示位置 0 が元の行 2、1 が元の行 0、2 が元の行 1
        let order = [2u64, 0, 1];
        let map = |p: u64| order.get(p as usize).copied();
        assert_eq!(row_ref(0, 3, map), Some(RowRef::Stored(2)));
        assert_eq!(row_ref(2, 3, map), Some(RowRef::Stored(1)));
        assert_eq!(row_ref(3, 3, map), Some(RowRef::Inserted(0)));
        assert_eq!(row_ref(5, 3, map), Some(RowRef::Inserted(2)));
    }

    #[test]
    fn an_unmapped_stored_row_is_none() {
        assert_eq!(row_ref(1, 3, |_| None), None);
    }
}
