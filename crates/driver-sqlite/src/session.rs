use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Canceller, DriverError, ErrorKind, ExecSummary, Flow, Result, RowSink, ServerInfo, Session,
    Statement,
};
use rusqlite::InterruptHandle;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::worker::{Msg, Worker};

pub(crate) struct SqliteSession {
    worker: Worker,
    info: ServerInfo,
}

impl SqliteSession {
    pub fn open(path: &str) -> Result<Self> {
        let (worker, version) = Worker::open(path)?;
        Ok(Self {
            worker,
            info: ServerInfo { version },
        })
    }
}

struct SqliteCanceller(Arc<InterruptHandle>);

#[async_trait]
impl Canceller for SqliteCanceller {
    async fn cancel(&self) -> Result<()> {
        self.0.interrupt();
        Ok(())
    }
}

/// 打ち切った後、実行中のクエリが止まって、接続が空くのを待つ上限
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
impl Session for SqliteSession {
    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary> {
        let (tx, mut rx) = mpsc::channel(4);
        self.worker.query(stmt.sql.clone(), stmt.params.clone(), tx);
        let mut summary = ExecSummary::default();
        let mut has_result_set = false;

        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Columns(columns) => {
                    has_result_set = true;
                    if sink.columns(&columns) == Flow::Stop {
                        return self.stop(rx, summary).await;
                    }
                }
                Msg::Rows(batch) => {
                    for row in batch {
                        summary.rows_received += 1;
                        let cells: Vec<Option<&[u8]>> = row.iter().map(|c| c.as_deref()).collect();
                        let mut flow = sink.row(&cells);
                        if flow == Flow::Pause {
                            flow = match sink.resume().await {
                                Flow::Pause => Flow::Stop,
                                other => other,
                            };
                        }
                        if flow == Flow::Stop {
                            return self.stop(rx, summary).await;
                        }
                    }
                }
                Msg::Done { affected } => {
                    summary.affected_rows =
                        affected.or(has_result_set.then_some(summary.rows_received));
                    return Ok(summary);
                }
                Msg::Failed(e) => return Err(e),
            }
        }
        Err(DriverError::new(
            ErrorKind::Disconnected,
            "SQLite のスレッドが終了しました",
        ))
    }

    fn canceller(&self) -> Box<dyn Canceller> {
        Box::new(SqliteCanceller(Arc::clone(&self.worker.interrupt)))
    }

    async fn close(self: Box<Self>) -> Result<()> {
        drop(self); // スレッドを止めて、接続を閉じる
        Ok(())
    }
}

impl SqliteSession {
    /// 受信の打ち切り。実行中のクエリを止め、接続が空くまで残りを読み捨てる。
    async fn stop(
        &self,
        mut rx: mpsc::Receiver<Msg>,
        mut summary: ExecSummary,
    ) -> Result<ExecSummary> {
        self.worker.interrupt.interrupt();
        let _ =
            tokio::time::timeout(DRAIN_TIMEOUT, async { while rx.recv().await.is_some() {} }).await;
        summary.stopped_early = true;
        Ok(summary)
    }
}
