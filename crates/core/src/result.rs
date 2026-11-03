use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use driver_api::{ResultColumn, ValueClass};
use result_store::{
    Access, ResultStore, RowView, SortIndex, SortKind, SortOrder, Stats, StoreError,
};

use crate::edit::EditAccess;
use crate::types::{ResultId, TabId};

/// 1 つの結果(結果タブ 1 つ)への窓口。複製しても同じ結果を指す。
///
/// UI は表示範囲の行だけを [`with_row`](Self::with_row) で読む。この呼び出しはファイル I/O を
/// しない。退避済みの行は [`Access::Loading`] を返し、読み戻しが終わると
/// [`set_on_ready`](Self::set_on_ready) で通知される。
#[derive(Clone)]
pub struct ResultHandle(Arc<Inner>);

struct Inner {
    id: ResultId,
    tab: TabId,
    statement_index: usize,
    sql: String,
    columns: Vec<ResultColumn>,
    store: ResultStore,
    sorted: RwLock<Option<Sorted>>,
    /// 一部だけ受信して止まった(打ち切り、エラー、キャンセル)
    truncated: AtomicBool,
    access: EditAccess,
}

struct Sorted {
    column: usize,
    order: SortOrder,
    index: SortIndex,
}

impl std::fmt::Debug for ResultHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 値は出さない
        f.debug_struct("ResultHandle")
            .field("id", &self.0.id)
            .field("columns", &self.0.columns.len())
            .field("rows", &self.0.store.row_count())
            .finish()
    }
}

impl ResultHandle {
    pub(crate) fn new(
        id: ResultId,
        tab: TabId,
        statement_index: usize,
        sql: String,
        columns: Vec<ResultColumn>,
        store: ResultStore,
        access: EditAccess,
    ) -> Self {
        Self(Arc::new(Inner {
            id,
            tab,
            statement_index,
            sql,
            columns,
            store,
            sorted: RwLock::new(None),
            truncated: AtomicBool::new(false),
            access,
        }))
    }

    /// この結果を、グリッドで編集できるか。
    pub fn edit_access(&self) -> &EditAccess {
        &self.0.access
    }

    /// 表示上の `position` 番目の行の、結果ストア上の行番号(並べ替え前)。編集は、この番号で指す。
    pub fn source_row(&self, position: u64) -> Option<u64> {
        match self.sorted().as_ref() {
            Some(s) => s.index.source_row(position),
            None => (position < self.row_count()).then_some(position),
        }
    }

    pub fn id(&self) -> ResultId {
        self.0.id
    }

    pub fn tab(&self) -> TabId {
        self.0.tab
    }

    /// 実行した文の中での位置(0 始まり)
    pub fn statement_index(&self) -> usize {
        self.0.statement_index
    }

    pub fn sql(&self) -> &str {
        &self.0.sql
    }

    pub fn columns(&self) -> &[ResultColumn] {
        &self.0.columns
    }

    /// 受信済みの行数。受信中は増え続ける。
    pub fn row_count(&self) -> u64 {
        self.0.store.row_count()
    }

    /// 受信が終わった(打ち切りやエラーで止まった場合も含む)
    pub fn is_finished(&self) -> bool {
        self.0.store.is_complete()
    }

    /// 一部だけ受信して止まった。
    pub fn is_truncated(&self) -> bool {
        self.0.truncated.load(Ordering::Acquire)
    }

    /// 並べ替えられる。全件を受信するまでは、一部の行だけを並べ替えて全体が並んだように
    /// 見せないよう、できない。
    pub fn can_sort(&self) -> bool {
        self.is_finished() && !self.is_truncated()
    }

    pub fn stats(&self) -> Stats {
        self.0.store.stats()
    }

    pub fn sort_spec(&self) -> Option<(usize, SortOrder)> {
        self.sorted().as_ref().map(|s| (s.column, s.order))
    }

    /// 結果ストア上の行番号(並べ替え前)で、1 行を読む。編集で、元の値を読むために使う。
    pub fn with_source_row<R>(&self, row: u64, f: impl FnOnce(RowView<'_>) -> R) -> Access<R> {
        self.0.store.with_row(row, f)
    }

    /// 表示上の `position` 番目の行を読む。並べ替え中は、並べ替え後の順序で数える。
    pub fn with_row<R>(&self, position: u64, f: impl FnOnce(RowView<'_>) -> R) -> Access<R> {
        let source = match self.sorted().as_ref() {
            Some(s) => match s.index.source_row(position) {
                Some(row) => row,
                None => return Access::Unavailable,
            },
            None => position,
        };
        self.0.store.with_row(source, f)
    }

    /// 表示上の行を所有する値として読む。退避済みなら呼んだスレッドでファイルを読むため、
    /// UI スレッドでは使わない(エクスポートなど)。
    pub fn read_row_blocking(
        &self,
        position: u64,
    ) -> Result<Option<Vec<Option<Vec<u8>>>>, StoreError> {
        let source = match self.sorted().as_ref() {
            Some(s) => match s.index.source_row(position) {
                Some(row) => row,
                None => return Ok(None),
            },
            None => position,
        };
        self.0.store.read_row_blocking(source)
    }

    /// 退避した行の読み戻しが終わるたびに呼ばれる。画面の再描画の依頼に使う。
    pub fn set_on_ready(&self, f: impl Fn() + Send + Sync + 'static) {
        self.0.store.set_on_ready(f);
    }

    fn sorted(&self) -> std::sync::RwLockReadGuard<'_, Option<Sorted>> {
        self.0.sorted.read().unwrap_or_else(|e| e.into_inner())
    }

    // ---- core 内部 ----

    pub(crate) fn store(&self) -> &ResultStore {
        &self.0.store
    }

    pub(crate) fn mark_truncated(&self) {
        self.0.truncated.store(true, Ordering::Release);
    }

    pub(crate) fn sort_kind(&self, column: usize) -> Option<SortKind> {
        self.0.columns.get(column).map(|c| match c.class {
            ValueClass::Number => SortKind::Numeric,
            _ => SortKind::Text,
        })
    }

    pub(crate) fn set_sorted(&self, column: usize, order: SortOrder, index: SortIndex) {
        *self.0.sorted.write().unwrap_or_else(|e| e.into_inner()) = Some(Sorted {
            column,
            order,
            index,
        });
    }

    pub(crate) fn clear_sorted(&self) {
        *self.0.sorted.write().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
