//! SQLite の接続を所有する専用スレッド。rusqlite は同期 API なので、Tokio のワーカーを塞がないよう、
//! 別のスレッドで動かし、結果をチャネルで流す(チャネルが満杯なら、スレッドが待つ。一時停止に相当)。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use driver_api::{DriverError, ResultColumn, ValueClass};
use rusqlite::{Connection, InterruptHandle};
use tokio::sync::mpsc as async_mpsc;

use crate::error::map_error;
use crate::values::{cell, classify, value_class};

/// 1 回に送る行数
const BATCH: usize = 256;

pub(crate) type Row = Vec<Option<Vec<u8>>>;

pub(crate) enum Msg {
    Columns(Vec<ResultColumn>),
    Rows(Vec<Row>),
    /// 完了。結果セットを返さない文では、影響行数
    Done {
        affected: Option<u64>,
    },
    Failed(DriverError),
}

enum Job {
    Query {
        sql: String,
        params: Vec<Option<String>>,
        out: async_mpsc::Sender<Msg>,
    },
    Close,
}

pub(crate) struct Worker {
    tx: mpsc::Sender<Job>,
    pub interrupt: Arc<InterruptHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn open(path: &str) -> Result<(Self, String), DriverError> {
        let conn = Connection::open(path).map_err(|e| map_error(e, ""))?;
        // 外部キーの制約は、接続ごとに有効にしないと働かない
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(|e| map_error(e, ""))?;
        let version: String = conn
            .query_row("select sqlite_version()", [], |r| r.get(0))
            .map_err(|e| map_error(e, ""))?;
        let interrupt = Arc::new(conn.get_interrupt_handle());
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = thread::Builder::new()
            .name("sqlite-worker".into())
            .spawn(move || run(conn, rx))
            .map_err(|e| DriverError::new(driver_api::ErrorKind::Other, e.to_string()))?;
        Ok((
            Self {
                tx,
                interrupt,
                thread: Some(thread),
            },
            format!("SQLite {version}"),
        ))
    }

    pub fn query(&self, sql: String, params: Vec<Option<String>>, out: async_mpsc::Sender<Msg>) {
        let _ = self.tx.send(Job::Query { sql, params, out });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Close);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(conn: Connection, jobs: mpsc::Receiver<Job>) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Close => return,
            Job::Query { sql, params, out } => {
                if let Err(e) = execute(&conn, &sql, &params, &out) {
                    let _ = out.blocking_send(Msg::Failed(map_error(e, &sql)));
                }
            }
        }
    }
}

fn execute(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
    out: &async_mpsc::Sender<Msg>,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let bind = rusqlite::params_from_iter(params.iter().map(|p| p.as_deref()));
    if stmt.column_count() == 0 {
        let n = stmt.execute(bind)?;
        let _ = out.blocking_send(Msg::Done {
            affected: Some(n as u64),
        });
        return Ok(());
    }
    let mut columns: Vec<ResultColumn> = stmt
        .columns()
        .map(|c| ResultColumn {
            name: c.name().to_string(),
            type_name: c.decl_type().map(str::to_string),
            class: classify(c.decl_type()),
        })
        .collect();
    let ncols = columns.len();
    let mut rows = stmt.query(bind)?;
    let to_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Row> {
        (0..ncols).map(|i| row.get_ref(i).map(cell)).collect()
    };
    let mut batch: Vec<Row> = Vec::with_capacity(BATCH);
    // 式の列には宣言された型がないので、最初の行の実際の値から分類する
    // (数値を、文字列として並べ替えてしまわないため)
    if let Some(first) = rows.next()? {
        for (i, c) in columns.iter_mut().enumerate() {
            if c.type_name.is_none() {
                c.class = first.get_ref(i).map_or(ValueClass::Other, value_class);
            }
        }
        batch.push(to_row(first)?);
    }
    if out.blocking_send(Msg::Columns(columns)).is_err() {
        return Ok(());
    }
    while let Some(row) = rows.next()? {
        batch.push(to_row(row)?);
        if batch.len() == BATCH
            && out
                .blocking_send(Msg::Rows(std::mem::take(&mut batch)))
                .is_err()
        {
            return Ok(()); // 受け取り側が止まった(打ち切り)
        }
    }
    if !batch.is_empty() {
        let _ = out.blocking_send(Msg::Rows(batch));
    }
    let _ = out.blocking_send(Msg::Done { affected: None });
    Ok(())
}
