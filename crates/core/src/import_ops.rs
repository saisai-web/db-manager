//! CSV の取り込み。全体を 1 つのトランザクションで、複数行の INSERT をバインドで実行する。

use std::path::PathBuf;
use std::sync::Arc;

use data_io::{CsvReader, IoError};
use driver_api::{ColumnInfo, Session, Statement};
use tokio::sync::mpsc;

use crate::core::{Res, Shared};
use crate::tabs::NullSink;
use crate::types::*;

type Batch = Vec<Vec<Option<String>>>;

/// 1 つの文に載せる行数の上限と、バインド値の数の上限(PostgreSQL は 65,535)
const BATCH_ROWS: usize = 500;
const MAX_PARAMS: usize = 60_000;

impl Shared {
    pub(crate) async fn preview_csv(
        self: &Arc<Self>,
        path: PathBuf,
        options: CsvReadOptions,
    ) -> Res {
        let p = path.clone();
        let data = tokio::task::spawn_blocking(move || CsvReader::preview(&p, &options, 20))
            .await
            .map_err(|e| UserError::new(e.to_string()))?
            .map_err(|e| UserError::new(e.to_string()))?;
        self.emit(Event::CsvPreview {
            path,
            encoding: data.encoding,
            delimiter: data.delimiter,
            headers: data.headers,
            rows: data.rows,
        });
        Ok(())
    }

    pub(crate) async fn import_csv(
        self: &Arc<Self>,
        tab_id: TabId,
        table: TableRef,
        path: PathBuf,
        options: CsvReadOptions,
        mapping: Vec<(usize, String)>,
    ) -> Res {
        let tab = self.tab(tab_id)?;
        {
            let rt = tab.rt();
            if rt.read_only {
                return Err(UserError::new("読み取り専用モードでは取り込めません"));
            }
            if rt.txn.is_pending() {
                return Err(UserError::new(
                    "未コミットのトランザクションがあります。COMMIT か ROLLBACK してから取り込んでください",
                ));
            }
        }
        let columns = self.mapped_columns(&tab, &table, &mapping).await?;
        {
            let mut rt = tab.rt();
            if rt.run != RunState::Idle {
                return Err(UserError::new("実行中のため、取り込めません"));
            }
            rt.run = RunState::Running;
            rt.cancel_requested = false;
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Running,
        });

        let result = self
            .run_import(&tab, &table, path, options, &mapping, &columns)
            .await;

        {
            let mut rt = tab.rt();
            rt.run = RunState::Idle;
            rt.cancel_requested = false;
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Idle,
        });
        self.emit(match result {
            Ok(rows) => Event::ImportFinished { tab: tab_id, rows },
            Err(error) => Event::ImportFailed { tab: tab_id, error },
        });
        Ok(())
    }

    /// 対応づけた CSV の列に、テーブルの列の情報(型)を結びつける。
    async fn mapped_columns(
        self: &Arc<Self>,
        tab: &Arc<crate::tab::Tab>,
        table: &TableRef,
        mapping: &[(usize, String)],
    ) -> Result<Vec<ColumnInfo>, UserError> {
        if mapping.is_empty() {
            return Err(UserError::new("取り込む列が指定されていません"));
        }
        let connection = tab.connection.id.clone();
        let cache = self.schema_cache(&connection);
        let details = cache.details(&self.connector(connection), table).await?;
        mapping
            .iter()
            .map(|(_, name)| {
                details
                    .columns
                    .iter()
                    .find(|c| &c.name == name)
                    .cloned()
                    .ok_or_else(|| UserError::new(format!("列「{name}」はテーブルにありません")))
            })
            .collect()
    }

    async fn run_import(
        self: &Arc<Self>,
        tab: &Arc<crate::tab::Tab>,
        table: &TableRef,
        path: PathBuf,
        options: CsvReadOptions,
        mapping: &[(usize, String)],
        columns: &[ColumnInfo],
    ) -> Result<u64, UserError> {
        let batch_rows = BATCH_ROWS.min(MAX_PARAMS / columns.len().max(1)).max(1);
        let (tx, mut rx) = mpsc::channel::<Result<Batch, IoError>>(4);
        let indexes: Vec<usize> = mapping.iter().map(|(i, _)| *i).collect();
        // CSV の読み取り(ファイル I/O と文字コードの変換)は、別のスレッドで行う
        let reader = tokio::task::spawn_blocking(move || {
            read_batches(&path, &options, &indexes, batch_rows, &tx)
        });

        let mut guard = tab.session.lock().await;
        let session = guard
            .as_mut()
            .ok_or_else(|| UserError::new("接続が切れています。再接続してください"))?;
        let d = tab.driver.dialect();
        session
            .execute(&Statement::new(d.begin_sql()), &mut NullSink)
            .await?;

        let mut done = 0u64;
        let failure = loop {
            let Some(batch) = rx.recv().await else {
                break None;
            };
            let batch = match batch {
                Ok(b) => b,
                Err(e) => break Some(UserError::new(e.to_string())),
            };
            if tab.rt().cancel_requested {
                break Some(UserError::new("取り込みを中止しました"));
            }
            match self
                .insert_batch(session.as_mut(), d, table, columns, &batch)
                .await
            {
                Ok(()) => {
                    done += batch.len() as u64;
                    self.emit(Event::ImportProgress {
                        tab: tab.id,
                        rows: done,
                    });
                }
                Err(mut e) => {
                    e.message = format!(
                        "{} 〜 {} 行目のどこかで失敗しました: {}",
                        done + 1,
                        done + batch.len() as u64,
                        e.message
                    );
                    break Some(e);
                }
            }
        };
        drop(rx);
        let _ = reader.await;

        match failure {
            None => {
                session
                    .execute(&Statement::new(d.commit_sql()), &mut NullSink)
                    .await?;
                Ok(done)
            }
            Some(e) => {
                let _ = session
                    .execute(&Statement::new(d.rollback_sql()), &mut NullSink)
                    .await;
                Err(e)
            }
        }
    }

    async fn insert_batch(
        &self,
        session: &mut dyn Session,
        d: &dyn driver_api::Dialect,
        table: &TableRef,
        columns: &[ColumnInfo],
        batch: &Batch,
    ) -> Result<(), UserError> {
        let build_sql = || {
            let names: Vec<String> = columns.iter().map(|c| d.quote_ident(&c.name)).collect();
            let mut n = 0;
            let rows: Vec<String> = batch
                .iter()
                .map(|_| {
                    let cells: Vec<String> = columns
                        .iter()
                        .map(|c| {
                            n += 1;
                            d.bind_text(n, &c.type_name)
                        })
                        .collect();
                    format!("({})", cells.join(", "))
                })
                .collect();
            format!(
                "INSERT INTO {} ({}) VALUES {}",
                d.qualified_name(table),
                names.join(", "),
                rows.join(", ")
            )
        };
        let sql = build_sql();
        let params: Vec<Option<String>> = batch.iter().flatten().cloned().collect();
        let summary = session
            .execute(&Statement::with_params(sql, params), &mut NullSink)
            .await?;
        match summary.affected_rows {
            Some(n) if n == batch.len() as u64 => Ok(()),
            other => Err(UserError::new(format!(
                "{} 行を取り込むはずが、{} 行になりました",
                batch.len(),
                other.unwrap_or(0)
            ))),
        }
    }
}

/// CSV を、対応づけた列だけに絞って、`rows` 行ずつ送る。
fn read_batches(
    path: &std::path::Path,
    options: &CsvReadOptions,
    indexes: &[usize],
    rows: usize,
    tx: &mpsc::Sender<Result<Batch, IoError>>,
) {
    let mut reader = match CsvReader::open(path, options) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.blocking_send(Err(e));
            return;
        }
    };
    let mut batch: Batch = Vec::with_capacity(rows);
    loop {
        match reader.next_record() {
            Ok(Some(rec)) => {
                batch.push(
                    indexes
                        .iter()
                        .map(|&i| rec.get(i).cloned().flatten())
                        .collect(),
                );
                if batch.len() == rows && tx.blocking_send(Ok(std::mem::take(&mut batch))).is_err()
                {
                    return; // 受け取り側が止まった(失敗か、中止)
                }
            }
            Ok(None) => {
                if !batch.is_empty() {
                    let _ = tx.blocking_send(Ok(batch));
                }
                return;
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
                return;
            }
        }
    }
}
