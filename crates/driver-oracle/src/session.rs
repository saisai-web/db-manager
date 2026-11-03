use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use driver_api::{
    Canceller, ConnectionConfig, DriverError, ErrorKind, ExecSummary, Flow, PlanFormat, Result,
    RowSink, ServerInfo, Session, Statement,
};
use tokio::sync::mpsc;

use crate::error::map_error;
use crate::worker::{Msg, Worker};
use crate::{KILL_CONNECT_TIMEOUT, open_blocking};

pub(crate) struct OracleSession {
    pub(crate) worker: Worker,
    info: ServerInfo,
    cfg: ConnectionConfig,
    sid: usize,
    serial: usize,
}

impl OracleSession {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let owned = cfg.clone();
        let (conn, version, sid, serial) = tokio::task::spawn_blocking(move || {
            let conn = open_blocking(&owned)?;
            let version = conn.version().map(|v| v.to_string()).unwrap_or_default();
            let sid = conn.session_id().map_err(|e| map_error(&e, ""))?;
            let serial = conn.serial_num().map_err(|e| map_error(&e, ""))?;
            Ok::<_, DriverError>((conn, version, sid, serial))
        })
        .await
        .map_err(|e| DriverError::new(ErrorKind::Other, e.to_string()))??;
        Ok(Self {
            worker: Worker::start(conn)?,
            info: ServerInfo {
                version: format!("Oracle Database {version}"),
            },
            cfg: cfg.clone(),
            sid,
            serial,
        })
    }
}

/// 実行中の文を止める。API がないので、別の接続から、このセッションを終了させる。
struct Killer {
    cfg: ConnectionConfig,
    sid: usize,
    serial: usize,
    killed: Arc<AtomicBool>,
}

const ORA_SESSION_MARKED_FOR_KILL: &str = "ORA-00031";
const ORA_INSUFFICIENT_PRIVILEGES: &str = "ORA-01031";

#[async_trait]
impl Canceller for Killer {
    async fn cancel(&self) -> Result<()> {
        let cfg = self.cfg.clone();
        let sql = format!(
            "ALTER SYSTEM KILL SESSION '{},{}' IMMEDIATE",
            self.sid, self.serial
        );
        // 実行側が、接続の切断を、キャンセルとして扱えるよう、先に印を付ける
        self.killed.store(true, Ordering::SeqCst);
        let result = tokio::time::timeout(
            KILL_CONNECT_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                let conn = open_blocking(&cfg)?;
                conn.execute(&sql, &[]).map_err(|e| map_error(&e, ""))?;
                Ok::<(), DriverError>(())
            }),
        )
        .await;
        let outcome = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => Err(DriverError::new(ErrorKind::Other, e.to_string())),
            Err(_) => Err(DriverError::new(
                ErrorKind::Connect,
                "停止の要求を送れませんでした",
            )),
        };
        match outcome {
            // IMMEDIATE のとき、成功しても「終了の印を付けた」というエラーが返る
            Err(e) if e.code.as_deref() == Some(ORA_SESSION_MARKED_FOR_KILL) => Ok(()),
            Err(e) => {
                self.killed.store(false, Ordering::SeqCst);
                if e.code.as_deref() == Some(ORA_INSUFFICIENT_PRIVILEGES) {
                    return Err(DriverError::new(
                        ErrorKind::Unsupported,
                        "Oracle では、実行中の文を止めるのに ALTER SYSTEM 権限が必要です(権限がないため、止められません)",
                    ));
                }
                Err(e)
            }
            Ok(()) => Ok(()),
        }
    }
}

#[async_trait]
impl Session for OracleSession {
    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary> {
        let (tx, mut rx) = mpsc::channel(4);
        self.worker.run(stmt.sql.clone(), stmt.params.clone(), tx);
        let mut summary = ExecSummary::default();
        let mut has_result_set = false;

        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Columns(columns) => {
                    has_result_set = true;
                    if sink.columns(&columns) == Flow::Stop {
                        return Ok(stopped(summary));
                    }
                }
                Msg::Rows(batch) => {
                    for row in batch {
                        summary.rows_received += 1;
                        let cells: Vec<Option<&[u8]>> = row.iter().map(|c| c.as_deref()).collect();
                        let mut flow = sink.row(&cells);
                        if flow == Flow::Pause {
                            flow = match sink.resume().await {
                                Flow::Pause => Flow::Stop, // 契約違反。安全側に倒して打ち切る
                                other => other,
                            };
                        }
                        if flow == Flow::Stop {
                            // 受け取りをやめると、スレッドは次の送信で気づいて、カーソルを閉じる
                            return Ok(stopped(summary));
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
            "Oracle のスレッドが終了しました",
        ))
    }

    async fn set_read_only(&mut self, read_only: bool) -> Result<()> {
        self.worker
            .mode
            .read_only
            .store(read_only, Ordering::SeqCst);
        Ok(())
    }

    async fn explain(&mut self, sql: &str, analyze: bool) -> Result<Option<(String, PlanFormat)>> {
        crate::explain::explain(self, sql, analyze).await.map(Some)
    }

    fn canceller(&self) -> Box<dyn Canceller> {
        Box::new(Killer {
            cfg: self.cfg.clone(),
            sid: self.sid,
            serial: self.serial,
            killed: Arc::clone(&self.worker.killed),
        })
    }

    async fn close(self: Box<Self>) -> Result<()> {
        drop(self); // 実行中の文があっても待たない(終わり次第、接続が閉じる)
        Ok(())
    }
}

fn stopped(mut summary: ExecSummary) -> ExecSummary {
    summary.stopped_early = true;
    summary.affected_rows = None;
    summary
}
