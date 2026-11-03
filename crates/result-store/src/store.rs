//! 結果ストアの公開 API。追記、表示用の読み出し、ソート。

use std::io;
use std::sync::{Arc, Mutex};

use crate::chunk::{Chunk, RowView};
use crate::engine::{Inner, Job, Loc, State};
use crate::sort::{self, SortIndex, SortKind, SortOrder};
use crate::types::{Access, Pressure, Stats, StoreConfig, StoreError};

/// 結果 1 つぶんのストア。複製しても同じストアを指す。
/// 最後の複製が破棄されると、メモリと一時ファイルを解放する。
#[derive(Clone)]
pub struct ResultStore(Arc<Inner>);

impl std::fmt::Debug for ResultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultStore")
            .field("cols", &self.0.ncols)
            .field("stats", &self.stats())
            .finish()
    }
}

impl ResultStore {
    pub fn new(ncols: usize, cfg: StoreConfig) -> Self {
        let state = State {
            open: Chunk::new(0, ncols),
            slots: Vec::new(),
            sealed_rows: 0,
            resident_bytes: 0,
            pending_write_bytes: 0,
            spilled_bytes: 0,
            tick: 0,
            complete: false,
            file: None,
            io: None,
            failure: None,
        };
        Self(Arc::new(Inner {
            ncols,
            cfg,
            state: Mutex::new(state),
            on_ready: Mutex::new(None),
        }))
    }

    pub fn col_count(&self) -> usize {
        self.0.ncols
    }

    pub fn row_count(&self) -> u64 {
        let st = self.0.lock();
        st.sealed_rows + st.open.row_count as u64
    }

    pub fn is_complete(&self) -> bool {
        self.0.lock().complete
    }

    pub fn stats(&self) -> Stats {
        let st = self.0.lock();
        Stats {
            rows: st.sealed_rows + st.open.row_count as u64,
            resident_bytes: st.resident_bytes + st.open.size_bytes(),
            spilled_bytes: st.spilled_bytes,
            pending_write_bytes: st.pending_write_bytes,
        }
    }

    /// 退避した行の読み戻しが終わるたびに、読み戻しスレッドから呼ばれる。
    /// 画面の再描画の依頼に使う。コールバック内でストアを操作してはならない。
    pub fn set_on_ready(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.0.on_ready.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// 行を末尾に追加する。`None` が NULL(空文字列とは別の値)。
    ///
    /// 失敗した場合は何も追加されず、それまでの行は読める。
    pub fn push_row<'a>(
        &self,
        cells: impl IntoIterator<Item = Option<&'a [u8]>>,
    ) -> Result<Pressure, StoreError> {
        let inner = &self.0;
        let mut st = inner.lock();
        if st.complete {
            return Err(StoreError::Finished);
        }
        if let Some((kind, msg)) = &st.failure {
            return Err(io::Error::new(*kind, msg.clone()).into());
        }
        if inner.cfg.spill_dir.is_none()
            && st.resident_bytes + st.open.size_bytes() >= inner.cfg.memory_limit
        {
            return Err(StoreError::LimitReached);
        }

        st.open.push_row(cells)?;
        if st.open.row_count >= inner.cfg.chunk_rows
            || st.open.data_len() >= inner.cfg.chunk_max_bytes
        {
            inner.seal_open(&mut st);
        }
        let high = st.pending_write_bytes > (inner.cfg.memory_limit / 2).max(1);
        Ok(if high {
            Pressure::High
        } else {
            Pressure::Normal
        })
    }

    /// 受信の終了。受信中のチャンクを封印し、ソートできるようにする。
    pub fn finish(&self) {
        let mut st = self.0.lock();
        if st.complete {
            return;
        }
        self.0.seal_open(&mut st);
        st.complete = true;
    }

    /// 表示用に 1 行を読む。ファイル I/O は行わない。
    ///
    /// `f` はロックを保持した状態で呼ばれることがあるため、短く保ち、
    /// このストアを操作しないこと。
    pub fn with_row<R>(&self, row: u64, f: impl FnOnce(RowView<'_>) -> R) -> Access<R> {
        let inner = &self.0;
        let mut st = inner.lock();
        match st.locate(row) {
            Loc::Missing => Access::Unavailable,
            Loc::Open => {
                let view = RowView {
                    chunk: &st.open,
                    row: (row - st.open.first_row) as usize,
                };
                Access::Ready(f(view))
            }
            Loc::Slot(i) => {
                st.tick += 1;
                let tick = st.tick;
                let slot = &mut st.slots[i];
                slot.last_used = tick;
                if let Some(chunk) = slot.mem.clone() {
                    let first_row = slot.first_row;
                    drop(st);
                    let view = RowView {
                        chunk: &chunk,
                        row: (row - first_row) as usize,
                    };
                    return Access::Ready(f(view));
                }
                if slot.failed {
                    return Access::Unavailable;
                }
                if !slot.loading {
                    slot.loading = true;
                    if !inner.send_job(&mut st, Job::Read { slot: i }) {
                        st.slots[i].loading = false;
                        st.slots[i].failed = true;
                        return Access::Unavailable;
                    }
                }
                Access::Loading
            }
        }
    }

    /// 1 行を所有する値として読む。退避済みなら呼び出したスレッドでファイルを読むため、
    /// UI スレッドでは使わない(エクスポートや検証用)。
    pub fn read_row_blocking(&self, row: u64) -> Result<Option<Vec<Option<Vec<u8>>>>, StoreError> {
        let loc = self.0.lock().locate(row);
        let collect = |chunk: &Chunk, r: usize| {
            (0..self.0.ncols)
                .map(|c| chunk.cell(r, c).map(<[u8]>::to_vec))
                .collect::<Vec<_>>()
        };
        match loc {
            Loc::Missing => Ok(None),
            Loc::Open => {
                let st = self.0.lock();
                let r = (row - st.open.first_row) as usize;
                Ok(Some(collect(&st.open, r)))
            }
            Loc::Slot(i) => {
                let chunk = self.0.chunk_blocking(i)?;
                Ok(Some(collect(&chunk, (row - chunk.first_row) as usize)))
            }
        }
    }

    /// 全件を受信した結果について、`col` 列の並び順を表す行番号のインデックスを作る。
    /// 元データは並べ替えない。受信途中の結果はクライアント側でソートしてはならないため、
    /// [`StoreError::NotComplete`] を返す。
    ///
    /// 呼び出したスレッドで一時ファイルを読むため、UI スレッドでは呼ばない。
    pub fn sort_index(
        &self,
        col: usize,
        order: SortOrder,
        kind: SortKind,
    ) -> Result<SortIndex, StoreError> {
        if col >= self.0.ncols {
            return Err(StoreError::ColumnOutOfRange(col));
        }
        let (slots, rows) = {
            let st = self.0.lock();
            if !st.complete {
                return Err(StoreError::NotComplete);
            }
            (st.slots.len(), st.sealed_rows)
        };
        if rows > u32::MAX as u64 {
            return Err(StoreError::TooLargeToSort);
        }
        let mut keys = sort::ColumnKeys::with_capacity(rows as usize);
        for i in 0..slots {
            let chunk = self.0.chunk_blocking(i)?;
            for r in 0..chunk.row_count as usize {
                if !keys.push(chunk.cell(r, col)) {
                    return Err(StoreError::TooLargeToSort);
                }
            }
        }
        Ok(keys.sort(order, kind))
    }
}
