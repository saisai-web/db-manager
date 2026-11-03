//! Oracle の接続を所有する専用スレッド。`oracledb` は同期 API なので、Tokio のワーカーを塞がないよう、
//! 別のスレッドで動かし、結果をチャネルで流す(チャネルが満杯なら、スレッドが待つ。一時停止に相当)。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use driver_api::{DriverError, ErrorKind, ResultColumn};
use oracledb::{Connection, ToDbValue};
use tokio::sync::mpsc as async_mpsc;

use crate::error::map_error;
use crate::txn::{Mode, Txn, is_alter_session, needs_read_only_begin};
use crate::values::{Kind, cell, column, kind};

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
    Run {
        sql: String,
        params: Vec<Option<String>>,
        out: async_mpsc::Sender<Msg>,
    },
    Close,
}

pub(crate) struct Worker {
    tx: mpsc::Sender<Job>,
    /// 別の接続から、このセッションを終了させた(キャンセル)
    pub killed: Arc<AtomicBool>,
    /// 読み取り専用モードと、明示的なトランザクションの状態(スレッドと共有)
    pub mode: Arc<Mode>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(conn: Connection) -> Result<Self, DriverError> {
        let killed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Job>();
        let flag = Arc::clone(&killed);
        let mode = Arc::new(Mode::default());
        let shared = Arc::clone(&mode);
        let thread = thread::Builder::new()
            .name("oracle-worker".into())
            .spawn(move || run(conn, rx, flag, shared))
            .map_err(|e| DriverError::new(ErrorKind::Other, e.to_string()))?;
        Ok(Self {
            tx,
            killed,
            mode,
            thread: Some(thread),
        })
    }

    pub fn run(&self, sql: String, params: Vec<Option<String>>, out: async_mpsc::Sender<Msg>) {
        let _ = self.tx.send(Job::Run { sql, params, out });
    }
}

impl Drop for Worker {
    /// スレッドの終了は待たない。実行中の文があると、その文が終わるまで終わらない
    /// (同期 API のため、途中で割り込めない)。終わったら、接続を閉じて、スレッドも終わる。
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Close);
        drop(self.thread.take());
    }
}

fn run(conn: Connection, jobs: mpsc::Receiver<Job>, killed: Arc<AtomicBool>, mode: Arc<Mode>) {
    let mut txn = Txn::default();
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Close => break,
            Job::Run { sql, params, out } => {
                let result = guarded(&conn, &sql, &params, &out, &mut txn, mode.read_only());
                mode.in_transaction.store(txn.explicit, Ordering::SeqCst);
                if let Err(failure) = result {
                    let mut err = match failure {
                        Failure::Db(e) => map_error(&e, &sql),
                        Failure::Rejected(e) => e,
                    };
                    // 別の接続から終了させたので、接続が切れたのではなく、キャンセル
                    if err.kind == ErrorKind::Disconnected && killed.swap(false, Ordering::SeqCst) {
                        err = DriverError::new(
                            ErrorKind::Cancelled,
                            "キャンセルしました(接続を終了しました)",
                        );
                    }
                    let _ = out.blocking_send(Msg::Failed(err));
                }
            }
        }
    }
    let mut conn = conn;
    let _ = conn.close();
}

/// 文末の `;` は、Oracle では要らない(付けるとエラーになる)。PL/SQL のブロックと、
/// ストアドの作成文は、`END;` の `;` が文の一部なので、そのまま渡す。
pub(crate) fn strip_terminator(sql: &str) -> &str {
    let t = sql.trim_end();
    let Some(body) = t.strip_suffix(';') else {
        return sql;
    };
    let head: String = body
        .trim_start()
        .chars()
        .take(40)
        .collect::<String>()
        .to_ascii_uppercase();
    let plsql = [
        "BEGIN",
        "DECLARE",
        "CREATE OR REPLACE",
        "CREATE PROCEDURE",
        "CREATE FUNCTION",
        "CREATE PACKAGE",
        "CREATE TRIGGER",
        "CREATE TYPE",
    ];
    if plsql.iter().any(|p| head.starts_with(p)) {
        sql
    } else {
        body.trim_end()
    }
}

enum Failure {
    Db(oracledb::Error),
    /// 実行する前に、こちらで断った(読み取り専用モードの `SET TRANSACTION`)
    Rejected(DriverError),
}

impl From<oracledb::Error> for Failure {
    fn from(e: oracledb::Error) -> Self {
        Self::Db(e)
    }
}

/// 読み取り専用モードでは、トランザクションの外の文を `SET TRANSACTION READ ONLY` で始め、
/// 終わったら必ず ROLLBACK する(成功しても失敗しても)。
fn guarded(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
    out: &async_mpsc::Sender<Msg>,
    txn: &mut Txn,
    read_only: bool,
) -> Result<(), Failure> {
    if !read_only {
        return execute(conn, sql, params, out, txn, false);
    }
    let begin = needs_read_only_begin(txn, sql).map_err(Failure::Rejected)?;
    let mut attempt = 0;
    loop {
        if begin {
            conn.execute("SET TRANSACTION READ ONLY", &[])?;
        }
        let result = execute(conn, sql, params, out, txn, true);
        // 明示的なトランザクションを始めた文(`begin_sql`)は、そのまま続ける
        if begin && !txn.explicit {
            let _ = conn.rollback();
        }
        // 作ったり変えたりした直後の表は、数秒のあいだ、読み取り専用のトランザクションから読めない
        // (ORA-01466)。少し待てば読めるので、結果を返す前なら、やり直す。
        match result {
            Err(Failure::Db(e)) if attempt < RETRIES && is_table_changed(&e) => {
                attempt += 1;
                thread::sleep(RETRY_WAIT);
            }
            // 待っても読めなかった。理由を添える
            Err(Failure::Db(e)) if is_table_changed(&e) => {
                let mut err = map_error(&e, sql);
                err.hint = Some(
                    "作成や変更をした直後の表は、数秒のあいだ、読み取り専用モードでは読めません。少し待ってやり直してください"
                        .into(),
                );
                return Err(Failure::Rejected(err));
            }
            other => return other,
        }
    }
}

const RETRIES: u32 = 5;
const RETRY_WAIT: Duration = Duration::from_millis(1000);
const ORA_TABLE_DEFINITION_CHANGED: usize = 1466;

fn is_table_changed(e: &oracledb::Error) -> bool {
    matches!(e.kind(), oracledb::ErrorKind::DbError(db) if db.code() == ORA_TABLE_DEFINITION_CHANGED)
}

fn execute(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
    out: &async_mpsc::Sender<Msg>,
    txn: &mut Txn,
    read_only: bool,
) -> Result<(), Failure> {
    let mut stmt = conn.statement(strip_terminator(sql))?.build()?;
    // DDL は、実行の前に、読み取り専用のトランザクションを暗黙にコミットして終わらせてしまう。
    // PL/SQL のブロックも、動的な DDL(`EXECUTE IMMEDIATE`)で同じことができる。どちらも断る
    if read_only && ((stmt.is_ddl() && !is_alter_session(sql)) || stmt.is_plsql()) {
        return Err(Failure::Rejected(DriverError::new(
            ErrorKind::Query,
            "読み取り専用モードでは、DDL と PL/SQL のブロックは実行できません",
        )));
    }
    let binds: Vec<&dyn ToDbValue> = params.iter().map(|p| p as &dyn ToDbValue).collect();
    if !stmt.is_query() {
        let result = stmt.execute(&binds)?;
        let affected = if stmt.is_plsql() {
            0
        } else {
            result.rows_affected()
        };
        // `ALTER SESSION` は DDL だが、暗黙にコミットしない
        txn.after(sql, stmt.is_ddl() && !is_alter_session(sql));
        if !read_only && !txn.explicit && (stmt.is_dml() || stmt.is_plsql()) {
            conn.commit()?;
        }
        let _ = out.blocking_send(Msg::Done {
            affected: Some(affected),
        });
        return Ok(());
    }

    let cursor = stmt.query(&binds)?;
    let metadata = cursor.columns().to_vec();
    let kinds: Vec<Kind> = metadata.iter().map(kind).collect();
    let type_names: Vec<String> = metadata.iter().map(|m| m.data_type()).collect();
    let columns: Vec<ResultColumn> = metadata.iter().map(column).collect();
    if out.blocking_send(Msg::Columns(columns)).is_err() {
        return Ok(());
    }
    let mut batch: Vec<Row> = Vec::with_capacity(BATCH);
    for row in cursor {
        let row = row?;
        batch.push(
            (0..kinds.len())
                .map(|i| cell(&row, i, kinds[i], &type_names[i]))
                .collect::<Result<Row, _>>()?,
        );
        if batch.len() == BATCH
            && out
                .blocking_send(Msg::Rows(std::mem::take(&mut batch)))
                .is_err()
        {
            return Ok(()); // 受け取り側が止まった(打ち切り)。カーソルを閉じて戻る
        }
    }
    if !batch.is_empty() && out.blocking_send(Msg::Rows(batch)).is_err() {
        return Ok(());
    }
    let _ = out.blocking_send(Msg::Done { affected: None });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_terminator;

    #[test]
    fn the_terminator_is_removed_except_for_plsql() {
        assert_eq!(
            strip_terminator("select 1 from dual;"),
            "select 1 from dual"
        );
        assert_eq!(
            strip_terminator("select 1 from dual ;  \n"),
            "select 1 from dual"
        );
        assert_eq!(strip_terminator("select 1 from dual"), "select 1 from dual");
        assert_eq!(strip_terminator("BEGIN NULL; END;"), "BEGIN NULL; END;");
        assert_eq!(
            strip_terminator("  declare x int; begin null; end;"),
            "  declare x int; begin null; end;"
        );
        assert_eq!(
            strip_terminator("create or replace procedure p is begin null; end;"),
            "create or replace procedure p is begin null; end;"
        );
        assert_eq!(
            strip_terminator("create table t (a int);"),
            "create table t (a int)"
        );
    }
}
