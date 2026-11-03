//! 読み取り専用モード。
//!
//! SQL Server には、セッションを読み取り専用にする仕組みが無い(`ApplicationIntent=ReadOnly` は
//! 可用性グループの振り分けの印で、書き込みを拒まない)。そこで、読み取り専用の間は、トランザクションの外の
//! 文を `BEGIN TRANSACTION` … 実行 … `ROLLBACK` で包み、文が書き込んでも、終わりに取り消す。
//! 書き込みの文は、まず core が拒否する。これは、その判定をすり抜けた場合の、サーバー側の備え。
//!
//! 限界: 取り消せない操作(シーケンスと IDENTITY の採番、`xp_cmdshell` などの外への作用、
//! `CREATE DATABASE` のようにトランザクション内で実行できない文)は、防げない。
//! 文の途中に `COMMIT` を含む複数文のバッチも、その時点で確定する。
//!
//! 利用者が `BEGIN TRANSACTION` した間は包まない。その間の `COMMIT` は、確定せずに取り消す。

use driver_api::{ExecSummary, Result, RowSink, Statement};
use futures_util::StreamExt;
use tiberius::QueryItem;

use crate::error::query_error;
use crate::session::SqlServerSession;

#[derive(PartialEq)]
enum Control {
    Begin,
    Commit,
    Rollback,
    Other,
}

fn control(sql: &str) -> Control {
    let mut words = sql
        .split_whitespace()
        .map(|w| w.trim_end_matches(';').to_ascii_uppercase());
    match (words.next().as_deref(), words.next().as_deref()) {
        (Some("BEGIN"), Some("TRAN" | "TRANSACTION" | "DISTRIBUTED")) => Control::Begin,
        (Some("COMMIT"), _) => Control::Commit,
        (Some("ROLLBACK"), _) => Control::Rollback,
        _ => Control::Other,
    }
}

pub(crate) async fn set(s: &mut SqlServerSession, read_only: bool) -> Result<()> {
    s.read_only = read_only;
    // 切り替える前から開いているトランザクションは、利用者のもの
    s.user_txn = read_only && s.open_transactions().await > 0;
    Ok(())
}

/// 読み取り専用モードでの 1 文の実行。
pub(crate) async fn execute(
    s: &mut SqlServerSession,
    stmt: &Statement,
    sink: &mut dyn RowSink,
) -> Result<ExecSummary> {
    match control(&stmt.sql) {
        Control::Begin => {
            let r = s.execute_inner(stmt, sink).await;
            if r.is_ok() {
                s.user_txn = true;
            }
            r
        }
        Control::Commit => {
            let r = s
                .execute_inner(&Statement::new("ROLLBACK TRANSACTION"), sink)
                .await;
            s.user_txn = s.open_transactions().await > 0;
            r
        }
        Control::Rollback => {
            let r = s.execute_inner(stmt, sink).await;
            s.user_txn = s.open_transactions().await > 0; // セーブポイントまでなら、まだ続く
            r
        }
        Control::Other if s.user_txn => s.execute_inner(stmt, sink).await,
        Control::Other => {
            s.run_plain("BEGIN TRANSACTION").await?;
            let r = s.execute_inner(stmt, sink).await;
            s.rollback_all().await;
            r
        }
    }
}

impl SqlServerSession {
    /// 結果を使わずに、設定などの文を実行する。
    pub(crate) async fn run_plain(&mut self, sql: &str) -> Result<()> {
        self.conn
            .simple_query(sql)
            .await
            .map_err(|e| query_error(&e, sql))?
            .into_results()
            .await
            .map_err(|e| query_error(&e, sql))?;
        Ok(())
    }

    /// 開いているトランザクションの入れ子の数。調べられなければ 0。
    pub(crate) async fn open_transactions(&mut self) -> i32 {
        let Ok(mut stream) = self.conn.simple_query("SELECT @@TRANCOUNT").await else {
            return 0;
        };
        while let Some(Ok(item)) = stream.next().await {
            if let QueryItem::Row(row) = item {
                return row.get::<i32, _>(0).unwrap_or(0);
            }
        }
        0
    }

    /// 開いているトランザクションを、すべて取り消す。失敗(切断など)は、接続ごと捨てられるので無視する。
    pub(crate) async fn rollback_all(&mut self) {
        let _ = self
            .run_plain("WHILE @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
            .await;
    }
}
