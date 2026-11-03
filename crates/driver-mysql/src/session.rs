use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Canceller, ConnectionConfig, DriverError, ErrorKind, ExecSummary, Flow, PlanFormat, Result,
    ResultColumn, RowSink, ServerInfo, Session, Statement,
};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, Params, Value};

use crate::error::{locate, map_error};
use crate::types::{classify, type_name, value_text};
use crate::{DRAIN_TIMEOUT, open, options};

pub(crate) struct MysqlSession {
    conn: Conn,
    info: ServerInfo,
    /// 停止の要求を別の接続から送るための、接続先と、この接続のスレッド番号
    opts: Opts,
    id: u32,
}

impl MysqlSession {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let mut conn = open(cfg).await?;
        let version: Option<String> = conn
            .query_first("SELECT VERSION()")
            .await
            .map_err(|e| map_error(&e))?;
        let id = conn.id();
        Ok(Self {
            conn,
            info: ServerInfo {
                version: version.unwrap_or_default(),
            },
            opts: options(cfg)?,
            id,
        })
    }

    fn killer(&self) -> Killer {
        Killer {
            opts: self.opts.clone(),
            id: self.id,
        }
    }
}

struct Killer {
    opts: Opts,
    id: u32,
}

#[async_trait]
impl Canceller for Killer {
    async fn cancel(&self) -> Result<()> {
        // 別の接続から、この接続で実行中の文だけを止める(接続は残る)
        let mut conn = tokio::time::timeout(Duration::from_secs(5), Conn::new(self.opts.clone()))
            .await
            .map_err(|_| DriverError::new(ErrorKind::Connect, "停止の要求を送れませんでした"))?
            .map_err(|e| crate::error::connect_error(&e))?;
        let r = conn
            .query_drop(format!("KILL QUERY {}", self.id))
            .await
            .map_err(|e| map_error(&e));
        let _ = conn.disconnect().await;
        r
    }
}

#[async_trait]
impl Session for MysqlSession {
    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary> {
        let killer = self.killer();
        if !stmt.params.is_empty() {
            return self.execute_bound(stmt).await;
        }
        let mut result = self
            .conn
            .query_iter(&stmt.sql)
            .await
            .map_err(|e| located(&e, &stmt.sql))?;

        let mut summary = ExecSummary::default();
        let mut saw_result_set = false;
        let mut flow = Flow::Continue;
        if let Some(cols) = result.columns().filter(|c| !c.is_empty()) {
            saw_result_set = true;
            let columns: Vec<ResultColumn> = cols
                .iter()
                .map(|c| ResultColumn {
                    name: c.name_str().into_owned(),
                    type_name: type_name(c),
                    class: classify(c),
                })
                .collect();
            flow = sink.columns(&columns);
        }

        loop {
            if flow == Flow::Pause {
                flow = match sink.resume().await {
                    Flow::Pause => Flow::Stop, // 契約違反。安全側に倒して打ち切る
                    other => other,
                };
            }
            if flow == Flow::Stop {
                summary.stopped_early = true;
                summary.affected_rows = None;
                let _ = killer.cancel().await;
                let _ = tokio::time::timeout(DRAIN_TIMEOUT, result.drop_result()).await;
                return Ok(summary);
            }
            let row = match result.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => return Err(located(&e, &stmt.sql)),
            };
            summary.rows_received += 1;
            let mut owned: Vec<Option<Vec<u8>>> = Vec::new();
            let cells: Vec<Option<&[u8]>> = (0..row.len())
                .map(|i| match row.as_ref(i) {
                    Some(Value::Bytes(b)) => Some(b.as_slice()),
                    Some(Value::NULL) | None => None,
                    // テキストプロトコルでは起こらない。起きても落とさず、文字列にする
                    Some(other) => {
                        owned.push(value_text(other));
                        None
                    }
                })
                .collect();
            flow = sink.row(&cells);
        }

        summary.affected_rows = if saw_result_set {
            Some(summary.rows_received)
        } else {
            Some(result.affected_rows())
        };
        // 複数の結果を返す文(CALL など)の、2 つ目以降は使わない
        let _ = result.drop_result().await;
        Ok(summary)
    }

    /// MySQL は木の文字列(`EXPLAIN FORMAT=TREE`、実測は `EXPLAIN ANALYZE`)、MariaDB は JSON
    /// (`EXPLAIN FORMAT=JSON`、実測は `ANALYZE FORMAT=JSON`)。同じドライバでも、サーバーで文が違う。
    async fn explain(&mut self, sql: &str, analyze: bool) -> Result<Option<(String, PlanFormat)>> {
        let maria = self.info.version.to_ascii_lowercase().contains("mariadb");
        let (prefix, format) = match (maria, analyze) {
            (true, false) => ("EXPLAIN FORMAT=JSON", PlanFormat::MariadbJson),
            (true, true) => ("ANALYZE FORMAT=JSON", PlanFormat::MariadbJson),
            (false, false) => ("EXPLAIN FORMAT=TREE", PlanFormat::MysqlTree),
            (false, true) => ("EXPLAIN ANALYZE", PlanFormat::MysqlTree),
        };
        let mut sink = FirstCell::default();
        self.execute(&Statement::new(format!("{prefix} {sql}")), &mut sink)
            .await?;
        sink.0
            .map(|text| Some((text, format)))
            .ok_or_else(|| DriverError::new(ErrorKind::Other, "実行計画が空です"))
    }

    fn canceller(&self) -> Box<dyn Canceller> {
        Box::new(self.killer())
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.conn.disconnect()).await;
        Ok(())
    }
}

impl MysqlSession {
    /// 値をバインドして実行する(編集と取り込みの INSERT / UPDATE / DELETE)。結果の行は使わない。
    async fn execute_bound(&mut self, stmt: &Statement) -> Result<ExecSummary> {
        let params: Vec<Value> = stmt
            .params
            .iter()
            .map(|p| {
                p.as_ref()
                    .map_or(Value::NULL, |s| Value::Bytes(s.clone().into_bytes()))
            })
            .collect();
        let result = self
            .conn
            .exec_iter(&stmt.sql, Params::Positional(params))
            .await
            .map_err(|e| located(&e, &stmt.sql))?;
        let affected = result.affected_rows();
        result.drop_result().await.map_err(|e| map_error(&e))?;
        Ok(ExecSummary {
            rows_received: 0,
            affected_rows: Some(affected),
            stopped_early: false,
        })
    }
}

fn located(e: &mysql_async::Error, sql: &str) -> DriverError {
    let mut err = map_error(e);
    locate(&mut err, sql);
    err
}

/// 先頭の 1 行 1 列を、文字列として受け取る。
#[derive(Default)]
struct FirstCell(Option<String>);

#[async_trait]
impl RowSink for FirstCell {
    fn columns(&mut self, _: &[ResultColumn]) -> Flow {
        Flow::Continue
    }

    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        if self.0.is_none() {
            self.0 = cells
                .first()
                .copied()
                .flatten()
                .map(|b| String::from_utf8_lossy(b).into_owned());
        }
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        Flow::Continue
    }
}
