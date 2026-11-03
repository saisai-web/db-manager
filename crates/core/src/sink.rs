use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use driver_api::{Flow, ResultColumn, RowSink};
use result_store::{Pressure, ResultStore, StoreConfig, StoreError};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::edit::EditAccess;
use crate::result::ResultHandle;
use crate::types::{Event, ResultId, TabId, TruncateReason};

/// 受信の一時停止と再開を、タブの状態へ反映する。`Some` で停止、`None` で再開。
pub(crate) type PauseHook = Arc<dyn Fn(Option<ResultId>) + Send + Sync>;

/// 画面の更新(60 Hz 程度)に合わせた、進捗通知の最短間隔
const PROGRESS_INTERVAL: Duration = Duration::from_millis(33);
/// 先頭のこの行数までは、行ごとに時刻を確かめる。最初の画面分を、完了を待たずに描画させるため
const PROGRESS_EAGER_ROWS: u64 = 1000;
/// 以降の時刻確認は、行ごとではなくこの行数ごと
const PROGRESS_CHECK_EVERY: u64 = 64;
/// 一時ファイルへの退避が追いつかないときに、受信を待たせる間隔
const BACKPRESSURE_WAIT: Duration = Duration::from_millis(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchCommand {
    NextBatch,
    Rest,
    Stop,
}

pub(crate) struct SinkParams {
    pub tab: TabId,
    pub statement_index: usize,
    pub sql: String,
    pub result_id: ResultId,
    pub store_config: StoreConfig,
    pub events: UnboundedSender<Event>,
    /// 受信を一時停止する行数。`u64::MAX` なら止めない
    pub fetch_limit: u64,
    pub control: UnboundedReceiver<FetchCommand>,
    /// 結果が開いたとき、core の一覧に登録する
    pub register: Arc<dyn Fn(ResultHandle) + Send + Sync>,
    pub on_pause: PauseHook,
    /// 結果が、グリッドで編集できるか(テーブルを開いた結果だけ)
    pub access: EditAccess,
}

#[derive(PartialEq, Eq)]
enum Wait {
    None,
    FetchLimit,
    Backpressure,
}

/// ドライバから行を受け取り、結果ストアへ追記する。
/// 取得上限での一時停止と、進捗の間引きもここで行う。
pub(crate) struct ResultSink {
    p: SinkParams,
    batch: u64,
    handle: Option<ResultHandle>,
    rows: u64,
    started: Instant,
    last_progress: Instant,
    wait: Wait,
    pub truncated: Option<TruncateReason>,
    pub failure: Option<StoreError>,
}

impl ResultSink {
    pub fn new(p: SinkParams) -> Self {
        let batch = p.fetch_limit;
        let now = Instant::now();
        Self {
            p,
            batch,
            handle: None,
            rows: 0,
            started: now,
            last_progress: now,
            wait: Wait::None,
            truncated: None,
            failure: None,
        }
    }

    pub fn result_id(&self) -> Option<ResultId> {
        self.handle.as_ref().map(|_| self.p.result_id)
    }

    /// 実行の判定(`outcome::classify`)に渡す、受信中に見たこと。
    pub fn report(&mut self) -> crate::outcome::SinkReport {
        crate::outcome::SinkReport {
            held_rows: self.handle.as_ref().map(|h| h.row_count()),
            store_failure: self.failure.take().map(|e| e.to_string()),
            truncated: self.truncated,
        }
    }

    /// 受信の終了(正常、打ち切り、エラーのいずれでも)。以降は並べ替えを許すかどうかだけが違う。
    pub fn finish(&mut self, complete: bool) {
        if let Some(h) = &self.handle {
            if !complete || self.truncated.is_some() || self.failure.is_some() {
                h.mark_truncated();
            }
            h.store().finish();
        }
    }
}

#[async_trait]
impl RowSink for ResultSink {
    fn columns(&mut self, columns: &[ResultColumn]) -> Flow {
        let store = ResultStore::new(columns.len(), self.p.store_config.clone());
        let handle = ResultHandle::new(
            self.p.result_id,
            self.p.tab,
            self.p.statement_index,
            self.p.sql.clone(),
            columns.to_vec(),
            store,
            self.p.access.clone(),
        );
        (self.p.register)(handle.clone());
        let _ = self.p.events.send(Event::ResultOpened {
            tab: self.p.tab,
            result: handle.clone(),
        });
        self.handle = Some(handle);
        Flow::Continue
    }

    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        let Some(handle) = &self.handle else {
            return Flow::Stop; // 列の通知より先に行が来ることはない
        };
        match handle.store().push_row(cells.iter().copied()) {
            Ok(Pressure::Normal) => {}
            Ok(Pressure::High) => self.wait = Wait::Backpressure,
            Err(StoreError::LimitReached) => {
                self.truncated = Some(TruncateReason::MemoryLimit);
                return Flow::Stop;
            }
            Err(e) => {
                self.failure = Some(e);
                return Flow::Stop;
            }
        }
        self.rows += 1;

        if self.rows >= self.p.fetch_limit {
            self.wait = Wait::FetchLimit;
            (self.p.on_pause)(Some(self.p.result_id));
            let _ = self.p.events.send(Event::FetchPaused {
                result: self.p.result_id,
                rows: self.rows,
            });
            return Flow::Pause;
        }
        if self.wait == Wait::Backpressure {
            return Flow::Pause;
        }
        if (self.rows <= PROGRESS_EAGER_ROWS || self.rows.is_multiple_of(PROGRESS_CHECK_EVERY))
            && self.last_progress.elapsed() >= PROGRESS_INTERVAL
        {
            self.last_progress = Instant::now();
            let _ = self.p.events.send(Event::Progress {
                result: self.p.result_id,
                rows: self.rows,
                elapsed: self.started.elapsed(),
            });
        }
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        match std::mem::replace(&mut self.wait, Wait::None) {
            Wait::Backpressure => {
                if let Some(h) = &self.handle {
                    // 退避の書き出しが、メモリ上限の 1/4 まで減るのを待つ
                    let low = self.p.store_config.memory_limit / 4;
                    while h.store().stats().pending_write_bytes > low {
                        tokio::time::sleep(BACKPRESSURE_WAIT).await;
                    }
                }
                Flow::Continue
            }
            Wait::FetchLimit => {
                let command = self.p.control.recv().await;
                (self.p.on_pause)(None);
                match command {
                    Some(FetchCommand::NextBatch) => {
                        self.p.fetch_limit = self.p.fetch_limit.saturating_add(self.batch);
                        Flow::Continue
                    }
                    Some(FetchCommand::Rest) => {
                        self.p.fetch_limit = u64::MAX;
                        Flow::Continue
                    }
                    Some(FetchCommand::Stop) | None => {
                        self.truncated = Some(TruncateReason::UserStopped);
                        Flow::Stop
                    }
                }
            }
            Wait::None => Flow::Continue,
        }
    }
}
