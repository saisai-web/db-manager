use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use driver_api::{
    Canceller, ConnectionConfig, DriverError, ErrorKind, ExecSummary, Flow, PlanFormat, Result,
    ResultColumn, RowSink, ServerInfo, Session, Statement,
};
use futures_util::{FutureExt, StreamExt};
use tiberius::{QueryItem, ToSql};
use tokio::sync::Notify;

use crate::error::{map_error, query_error};
use crate::values::{cell, classify, type_name};
use crate::{Conn, DRAIN_TIMEOUT, open};

/// 停止の要求。実行中の `execute` が受け取り、Attention を送る。
#[derive(Default)]
pub(crate) struct CancelSignal {
    pub(crate) requested: AtomicBool,
    pub(crate) notify: Notify,
}

pub(crate) struct SqlServerSession {
    pub(crate) conn: Conn,
    info: ServerInfo,
    pub(crate) cancel: Arc<CancelSignal>,
    /// 実行を止めたあと、接続を張り直すための接続先
    cfg: ConnectionConfig,
    /// 読み取り専用モード([`crate::readonly`])
    pub(crate) read_only: bool,
    /// 読み取り専用の間に、利用者が開いたトランザクションの中にいる
    pub(crate) user_txn: bool,
}

impl SqlServerSession {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let mut conn = open(cfg).await?;
        let row = conn
            .simple_query("SELECT @@VERSION")
            .await
            .map_err(|e| map_error(&e))?
            .into_row()
            .await
            .map_err(|e| map_error(&e))?;
        let version = row
            .and_then(|r| {
                r.get::<&str, _>(0)
                    .map(|s| s.lines().next().unwrap_or("").to_string())
            })
            .unwrap_or_default();
        Ok(Self {
            conn,
            info: ServerInfo { version },
            cancel: Arc::default(),
            cfg: cfg.clone(),
            read_only: false,
            user_txn: false,
        })
    }
}

struct Attention(Arc<CancelSignal>);

#[async_trait]
impl Canceller for Attention {
    async fn cancel(&self) -> Result<()> {
        self.0.requested.store(true, Ordering::SeqCst);
        self.0.notify.notify_one();
        Ok(())
    }
}

pub(crate) fn cancelled() -> DriverError {
    let mut e = DriverError::new(ErrorKind::Cancelled, "ユーザーの操作で中断されました");
    e.hint = Some(
        "SQL Server では、実行を止めるために接続を張り直します。未コミットのトランザクションがあれば、取り消されています"
            .into(),
    );
    e
}

#[async_trait]
impl Session for SqlServerSession {
    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary> {
        if self.read_only {
            return crate::readonly::execute(self, stmt, sink).await;
        }
        self.execute_inner(stmt, sink).await
    }

    async fn set_read_only(&mut self, read_only: bool) -> Result<()> {
        crate::readonly::set(self, read_only).await
    }

    async fn explain(&mut self, sql: &str, analyze: bool) -> Result<Option<(String, PlanFormat)>> {
        crate::explain::explain(self, sql, analyze).await.map(Some)
    }

    fn canceller(&self) -> Box<dyn Canceller> {
        Box::new(Attention(self.cancel.clone()))
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, self.conn.close()).await;
        Ok(())
    }
}

impl SqlServerSession {
    /// 読み取り専用の包みを付けずに、1 文を実行する。
    pub(crate) async fn execute_inner(
        &mut self,
        stmt: &Statement,
        sink: &mut dyn RowSink,
    ) -> Result<ExecSummary> {
        if !stmt.params.is_empty() {
            return self.execute_bound(stmt).await;
        }
        // 実行前に届いた古い停止の要求は捨てる
        self.cancel.requested.store(false, Ordering::SeqCst);
        let cancel = self.cancel.clone();
        let _ = cancel.notify.notified().now_or_never();
        let run = stream_statement(&mut self.conn, &cancel, stmt, sink).await?;
        let mut summary = run.summary;
        match run.end {
            End::Finished if run.saw_result_set => {
                summary.affected_rows = Some(summary.rows_received)
            }
            // 結果を返さない文の影響行数は、直前の文の `@@ROWCOUNT` で得る(DDL は 0)
            End::Finished => summary.affected_rows = self.last_row_count().await,
            End::Cancelled => {
                self.reconnect().await?;
                return Err(cancelled());
            }
            End::Stopped => {
                self.reconnect().await?;
                summary.stopped_early = true;
                summary.affected_rows = None;
            }
        }
        Ok(summary)
    }
}

/// 文の実行が終わった理由。
enum End {
    Finished,
    /// 利用者の停止の要求
    Cancelled,
    /// 受け取り側が打ち切った(取得上限での打ち切り)
    Stopped,
}

struct Run {
    summary: ExecSummary,
    saw_result_set: bool,
    end: End,
}

/// 1 文を実行し、結果を `sink` へ流す。接続を借りるのはこの関数の間だけで、
/// 止めた後の後始末(Attention)は、戻ってから呼び出し側が行う。
async fn stream_statement(
    conn: &mut Conn,
    cancel: &CancelSignal,
    stmt: &Statement,
    sink: &mut dyn RowSink,
) -> Result<Run> {
    let mut run = Run {
        summary: ExecSummary::default(),
        saw_result_set: false,
        end: End::Finished,
    };
    // 最初の結果が届くまで(長い問合せでは、ここで長く待つ)も、停止の要求を受け付ける
    let started = tokio::select! {
        s = conn.simple_query(stmt.sql.as_str()) => Some(s),
        () = cancel.notify.notified() => None,
    };
    let Some(started) = started else {
        run.end = End::Cancelled;
        return Ok(run);
    };
    let mut stream = started.map_err(|e| query_error(&e, &stmt.sql))?;
    let mut flow = Flow::Continue;
    let mut sets_seen = 0;
    loop {
        if flow == Flow::Pause {
            flow = match sink.resume().await {
                Flow::Pause => Flow::Stop, // 契約違反。安全側に倒して打ち切る
                other => other,
            };
        }
        if flow == Flow::Stop {
            run.end = End::Stopped;
            break;
        }
        let item = tokio::select! {
            item = stream.next() => item,
            () = cancel.notify.notified() => {
                run.end = End::Cancelled;
                break;
            }
        };
        let item = match item {
            None => break,
            Some(Err(e)) => return Err(query_error(&e, &stmt.sql)),
            Some(Ok(item)) => item,
        };
        match item {
            QueryItem::Metadata(meta) => {
                sets_seen += 1;
                if sets_seen > 1 {
                    continue; // 2 つ目以降の結果セットは使わない(読み捨てる)
                }
                run.saw_result_set = true;
                let columns: Vec<ResultColumn> = meta
                    .columns()
                    .iter()
                    .map(|c| ResultColumn {
                        name: c.name().to_string(),
                        type_name: type_name(c),
                        class: classify(c),
                    })
                    .collect();
                flow = sink.columns(&columns);
            }
            QueryItem::Row(row) => {
                if sets_seen > 1 {
                    continue;
                }
                run.summary.rows_received += 1;
                let owned: Vec<Option<Vec<u8>>> = row.cells().map(|(_, d)| cell(d)).collect();
                let cells: Vec<Option<&[u8]>> = owned.iter().map(|c| c.as_deref()).collect();
                flow = sink.row(&cells);
            }
        }
    }
    Ok(run)
}

impl SqlServerSession {
    /// 実行中の要求を止め、接続を次の文に使える状態へ戻す。
    ///
    /// tiberius の Attention は、サーバーが 2 通で返す応答(中断したバッチの終わりと、Attention の確認)を
    /// 1 通しか読まず、以降の応答が 1 つずつずれる。そのため、接続を閉じて張り直す。
    /// サーバーは切断でバッチを中止し、未コミットのトランザクションを取り消す。
    pub(crate) async fn reconnect(&mut self) -> Result<()> {
        self.conn = open(&self.cfg).await?;
        self.user_txn = false; // 切断で、トランザクションは取り消されている
        Ok(())
    }

    async fn last_row_count(&mut self) -> Option<u64> {
        let stream = self.conn.simple_query("SELECT @@ROWCOUNT").await.ok()?;
        let row = stream.into_row().await.ok()??;
        row.get::<i32, _>(0).map(|n| u64::try_from(n).unwrap_or(0))
    }

    /// 値をバインドして実行する(編集と取り込みの INSERT / UPDATE / DELETE)。結果の行は使わない。
    /// 値は文字列(nvarchar)で渡し、SQL 側の `CAST` で列の型にする。
    async fn execute_bound(&mut self, stmt: &Statement) -> Result<ExecSummary> {
        let params: Vec<Option<&str>> = stmt.params.iter().map(|p| p.as_deref()).collect();
        let refs: Vec<&dyn ToSql> = params.iter().map(|p| p as &dyn ToSql).collect();
        let result = self
            .conn
            .execute(stmt.sql.as_str(), &refs)
            .await
            .map_err(|e| query_error(&e, &stmt.sql))?;
        Ok(ExecSummary {
            rows_received: 0,
            affected_rows: Some(result.total()),
            stopped_early: false,
        })
    }
}
