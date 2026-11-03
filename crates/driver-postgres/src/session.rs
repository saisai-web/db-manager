use std::time::Duration;

use async_trait::async_trait;
use driver_api::{
    Canceller, ConnectionConfig, ExecSummary, Flow, Result, ResultColumn, RowSink, ServerInfo,
    Session, Statement, ValueClass,
};
use futures_util::StreamExt;
use tokio::task::JoinHandle;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage, types::Type};

use crate::error::{connect_error, map_error};
use crate::tls::Connector;
use crate::types::{classify, decode_bytea_hex};

/// 打ち切り後に、サーバーが止まって接続が空くのを待つ上限
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct PgSession {
    pub(crate) client: Client,
    task: JoinHandle<()>,
    info: ServerInfo,
    /// 暗号化の方法。キャンセルの要求にも使う
    connector: Connector,
}

impl PgSession {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let (client, task, connector) = crate::open(cfg).await?;
        let info = match server_version(&client).await {
            Ok(info) => info,
            Err(e) => {
                task.abort();
                return Err(e);
            }
        };
        Ok(Self {
            client,
            task,
            info,
            connector,
        })
    }

    /// 文を実行する前に Parse だけを行い、結果列の型を得る。
    /// 失敗しても実行側が同じ原因で、位置つきの正しいエラーを返すため、ここでは無視する。
    async fn describe(&self, sql: &str) -> Option<Vec<Type>> {
        let st = self.client.prepare(sql).await.ok()?;
        Some(st.columns().iter().map(|c| c.type_().clone()).collect())
    }
}

async fn server_version(client: &Client) -> Result<ServerInfo> {
    let messages = client
        .simple_query("SELECT version()")
        .await
        .map_err(connect_error)?;
    let version = messages
        .iter()
        .find_map(|m| match m {
            SimpleQueryMessage::Row(r) => r.get(0).map(str::to_string),
            _ => None,
        })
        .unwrap_or_default();
    Ok(ServerInfo { version })
}

struct PgCanceller {
    token: tokio_postgres::CancelToken,
    connector: Connector,
}

#[async_trait]
impl Canceller for PgCanceller {
    async fn cancel(&self) -> Result<()> {
        cancel(&self.token, &self.connector).await
    }
}

/// 停止の要求は、別の接続で送る。本体の接続と同じ暗号化の方法を使う。
async fn cancel(token: &tokio_postgres::CancelToken, connector: &Connector) -> Result<()> {
    match connector {
        Connector::Plain => token.cancel_query(NoTls).await,
        Connector::Tls(tls) => token.cancel_query(tls.clone()).await,
    }
    .map_err(connect_error)
}

#[async_trait]
impl Session for PgSession {
    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary> {
        if !stmt.params.is_empty() {
            return self.execute_bound(stmt).await;
        }
        let types = self.describe(&stmt.sql).await;
        let stream = self
            .client
            .simple_query_raw(&stmt.sql)
            .await
            .map_err(|e| map_error(&e))?;
        futures_util::pin_mut!(stream);

        let mut summary = ExecSummary::default();
        let mut classes: Vec<ValueClass> = Vec::new();
        let mut has_binary = false;
        let mut saw_result_set = false;

        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| map_error(&e))?;
            let mut flow = Flow::Continue;
            match msg {
                SimpleQueryMessage::RowDescription(cols) => {
                    saw_result_set = true;
                    let columns: Vec<ResultColumn> = cols
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            // 型の取得に失敗した、または列数が合わない場合は型を使わない
                            let ty = types
                                .as_ref()
                                .filter(|t| t.len() == cols.len())
                                .map(|t| &t[i]);
                            ResultColumn {
                                name: c.name().to_string(),
                                type_name: ty.map(|t| t.name().to_string()),
                                class: ty.map_or(ValueClass::Other, classify),
                            }
                        })
                        .collect();
                    classes = columns.iter().map(|c| c.class).collect();
                    has_binary = classes.contains(&ValueClass::Binary);
                    flow = sink.columns(&columns);
                }
                SimpleQueryMessage::Row(row) => {
                    summary.rows_received += 1;
                    let mut texts: Vec<Option<&str>> = Vec::with_capacity(classes.len());
                    for i in 0..classes.len() {
                        texts.push(row.try_get(i).map_err(|e| map_error(&e))?);
                    }
                    // バイナリ列は、16 進の文字列からバイト列へ戻す。復号できなければ文字列のまま
                    let decoded: Vec<Option<Vec<u8>>> = if has_binary {
                        classes
                            .iter()
                            .zip(&texts)
                            .map(|(c, t)| match (c, t) {
                                (ValueClass::Binary, Some(t)) => decode_bytea_hex(t),
                                _ => None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let cells: Vec<Option<&[u8]>> = texts
                        .iter()
                        .enumerate()
                        .map(|(i, t)| match decoded.get(i) {
                            Some(Some(bytes)) => Some(bytes.as_slice()),
                            _ => t.map(str::as_bytes),
                        })
                        .collect();
                    flow = sink.row(&cells);
                }
                SimpleQueryMessage::CommandComplete(n) => {
                    summary.affected_rows = Some(n);
                }
                _ => {}
            }

            if flow == Flow::Pause {
                flow = match sink.resume().await {
                    Flow::Pause => Flow::Stop, // 契約違反。安全側に倒して打ち切る
                    other => other,
                };
            }
            if flow == Flow::Stop {
                summary.stopped_early = true;
                summary.affected_rows = None;
                self.stop_and_drain(&mut stream).await;
                return Ok(summary);
            }
        }
        if saw_result_set && summary.affected_rows.is_none() {
            summary.affected_rows = Some(summary.rows_received);
        }
        Ok(summary)
    }

    fn canceller(&self) -> Box<dyn Canceller> {
        Box::new(PgCanceller {
            token: self.client.cancel_token(),
            connector: self.connector.clone(),
        })
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let Self { client, task, .. } = *self;
        drop(client); // 接続を閉じると、駆動タスクが終了する
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        Ok(())
    }
}

impl PgSession {
    /// 受信の打ち切り。サーバーに停止を依頼し、接続が空くまで残りを読み捨てる。
    /// 空かないまま次の文を送ると、前のクエリの終了を待たされるため。
    async fn stop_and_drain(
        &self,
        stream: &mut (
                 impl futures_util::Stream<
            Item = std::result::Result<SimpleQueryMessage, tokio_postgres::Error>,
        > + Unpin
             ),
    ) {
        let _ = cancel(&self.client.cancel_token(), &self.connector).await;
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
            while let Some(m) = stream.next().await {
                if m.is_err() {
                    break; // 停止による query_canceled
                }
            }
        })
        .await;
    }
}
