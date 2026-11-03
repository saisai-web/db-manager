//! 結果グリッド。表示範囲の行だけを `ResultHandle` から読み出す(100 万行でも行オブジェクトを作らない)。

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use sqltool_core::{Core, EditBuffer, ResultHandle, SortOrder, ValueClass};

use super::cell_text::{CellText, resolve, total_rows};
use super::prelude::*;
use super::theme;

pub struct GridDelegate {
    core: Core,
    handle: ResultHandle,
    /// 編集中の変更(テーブルを開いた結果だけ)
    edit: Option<Rc<RefCell<EditBuffer>>>,
    /// 先頭行から測った列幅。十分な行が届くか、受信が終わったら 1 度だけ確定する
    fitted: RefCell<Option<Vec<f32>>>,
}

/// 幅を測るのに見る行数
const SAMPLE_ROWS: u64 = 100;
const MIN_WIDTH: f32 = 80.;
const MAX_WIDTH: f32 = 520.;
const CHAR_WIDTH: f32 = 8.0;
const CELL_PADDING: f32 = 28.;

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if c as u32 >= 0x2E80 { 2 } else { 1 })
        .sum()
}

/// 退避した行の読み戻しが終わったら、グリッドを再描画する(読み戻しは別スレッドで行われる)。
pub(super) fn watch_reloads(
    handle: &ResultHandle,
    table: &Entity<TableState<GridDelegate>>,
    cx: &mut App,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    handle.set_on_ready(move || {
        let _ = tx.send(());
    });
    let table = table.downgrade();
    cx.spawn(async move |cx| {
        while rx.recv().await.is_some() {
            while rx.try_recv().is_ok() {} // 続けて来た通知は 1 回にまとめる
            if table.update(cx, |t, cx| t.refresh(cx)).is_err() {
                break; // 結果タブが閉じられた
            }
        }
    })
    .detach();
}

impl GridDelegate {
    pub fn new(core: Core, handle: ResultHandle, edit: Option<Rc<RefCell<EditBuffer>>>) -> Self {
        Self {
            core,
            handle,
            edit,
            fitted: RefCell::new(None),
        }
    }

    /// 列 `ix` の幅。先頭の行の内容と見出しから決める(受信が進むまでは、型ごとの既定値)。
    fn width_of(&self, ix: usize) -> f32 {
        let c = &self.handle.columns()[ix];
        let default = match c.class {
            ValueClass::Number => 150.,
            ValueClass::Bool => 90.,
            ValueClass::Temporal | ValueClass::Binary => 220.,
            _ => 180.,
        };
        let ready = self.handle.row_count() >= SAMPLE_ROWS || self.handle.is_finished();
        if !ready {
            return default;
        }
        let mut fitted = self.fitted.borrow_mut();
        let widths = fitted.get_or_insert_with(|| self.measure());
        widths.get(ix).copied().unwrap_or(default)
    }

    fn measure(&self) -> Vec<f32> {
        let n = self.handle.row_count().min(SAMPLE_ROWS) as usize;
        let mut chars: Vec<usize> = self
            .handle
            .columns()
            .iter()
            .map(|c| display_width(&c.name) + 3) // 並べ替えの印の分
            .collect();
        for row in 0..n {
            for (col, w) in chars.iter_mut().enumerate() {
                if let CellText::Text(t) = resolve(&self.handle, None, row, col).text {
                    *w = (*w).max(display_width(&t));
                }
            }
        }
        chars
            .into_iter()
            .map(|c| (c as f32 * CHAR_WIDTH + CELL_PADDING).clamp(MIN_WIDTH, MAX_WIDTH))
            .collect()
    }
}

impl TableDelegate for GridDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.handle.columns().len()
    }

    fn rows_count(&self, _: &App) -> usize {
        let edit = self.edit.as_ref().map(|e| e.borrow());
        total_rows(&self.handle, edit.as_deref())
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        let c = &self.handle.columns()[ix];
        let col = Column::new(format!("c{ix}"), c.name.clone()).width(self.width_of(ix));
        let col = if c.class == ValueClass::Number {
            col.text_right()
        } else {
            col
        };
        // 全件を受信するまでは並べ替えない(一部の行だけが並んだのを、全体が並んだと誤認させない)
        if self.handle.can_sort() {
            col.sortable()
        } else {
            col
        }
    }

    fn perform_sort(
        &mut self,
        ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        use sqltool_core::Command;
        let result = self.handle.id();
        self.core.send(match sort {
            ColumnSort::Ascending => Command::SortResult {
                result,
                column: ix,
                order: SortOrder::Ascending,
            },
            ColumnSort::Descending => Command::SortResult {
                result,
                column: ix,
                order: SortOrder::Descending,
            },
            _ => Command::ClearSort { result },
        });
    }

    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let edit = self.edit.as_ref().map(|e| e.borrow());
        let cell = resolve(&self.handle, edit.as_deref(), row, col);
        let right = self.handle.columns()[col].class == ValueClass::Number;
        let base = div().w_full().overflow_hidden().whitespace_nowrap();
        let base = if right { base.text_right() } else { base };
        let base = if cell.edited {
            base.bg(theme::edited_background())
        } else {
            base
        };
        let base = if cell.deleted {
            base.line_through().text_color(theme::error_red())
        } else {
            base
        };
        match cell.text {
            CellText::Text(t) => base.child(t),
            CellText::Null => base.text_color(muted).italic().child("NULL"),
            CellText::Empty => base.text_color(muted).child("''"),
            CellText::Default => base.text_color(muted).italic().child("(既定値)"),
            CellText::Loading => base.text_color(muted).child("読込中…"),
            CellText::Missing => base.text_color(muted).child("—"),
        }
    }

    fn cell_text(&self, row: usize, col: usize, _: &App) -> String {
        let edit = self.edit.as_ref().map(|e| e.borrow());
        match resolve(&self.handle, edit.as_deref(), row, col).text {
            CellText::Text(t) => t,
            _ => String::new(),
        }
    }
}
