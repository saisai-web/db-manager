//! 実行計画。推定は `SET SHOWPLAN_XML ON`(文は実行されず、計画の XML が返る)、
//! 実測は `SET STATISTICS XML ON`(文を実行し、結果のあとに実測つきの計画の XML が返る)。
//! どちらも、設定を元に戻してから返す(接続に残すと、以降の文が計画しか返さなくなる)。

use std::sync::atomic::Ordering;

use driver_api::{DriverError, ErrorKind, PlanFormat, Result};
use futures_util::{FutureExt, StreamExt};
use tiberius::QueryItem;

use crate::error::query_error;
use crate::session::{SqlServerSession, cancelled};
use crate::values::cell;

pub(crate) async fn explain(
    s: &mut SqlServerSession,
    sql: &str,
    analyze: bool,
) -> Result<(String, PlanFormat)> {
    let (on, off) = if analyze {
        ("SET STATISTICS XML ON", "SET STATISTICS XML OFF")
    } else {
        ("SET SHOWPLAN_XML ON", "SET SHOWPLAN_XML OFF")
    };
    // 実測は文を実行する。読み取り専用モードでは、取り消す包みの中で行う
    let wrapped = analyze && s.read_only && !s.user_txn;
    if wrapped {
        s.run_plain("BEGIN TRANSACTION").await?;
    }
    s.run_plain(on).await?;
    let result = collect_plan(s, sql).await;
    // 取り消しでは、接続を張り直してあり、設定も戻っている
    if !matches!(&result, Err(e) if e.kind == ErrorKind::Cancelled)
        && s.run_plain(off).await.is_err()
    {
        s.reconnect().await?;
    }
    if wrapped {
        s.rollback_all().await;
    }
    result.map(|xml| (xml, PlanFormat::SqlServerXml))
}

/// 文を実行し、返ってきた結果のうち、最初の実行計画の XML を取り出す。
async fn collect_plan(s: &mut SqlServerSession, sql: &str) -> Result<String> {
    s.cancel.requested.store(false, Ordering::SeqCst);
    let cancel = s.cancel.clone();
    let _ = cancel.notify.notified().now_or_never();
    let mut plan = None;
    let mut was_cancelled = false;
    {
        let started = tokio::select! {
            r = s.conn.simple_query(sql) => Some(r),
            () = cancel.notify.notified() => None,
        };
        match started {
            None => was_cancelled = true,
            Some(started) => {
                let mut stream = started.map_err(|e| query_error(&e, sql))?;
                loop {
                    let item = tokio::select! {
                        item = stream.next() => item,
                        () = cancel.notify.notified() => {
                            was_cancelled = true;
                            break;
                        }
                    };
                    match item {
                        None => break,
                        Some(Err(e)) => return Err(query_error(&e, sql)),
                        Some(Ok(QueryItem::Row(row))) if plan.is_none() => {
                            plan = row
                                .cells()
                                .next()
                                .and_then(|(_, d)| cell(d))
                                .map(|b| String::from_utf8_lossy(&b).into_owned())
                                .filter(|t| t.trim_start().starts_with("<ShowPlanXML"));
                            // `SET` などの文は、演算子の無い空の計画が返る
                            if plan.as_ref().is_some_and(|p| !p.contains("<RelOp")) {
                                plan = None;
                            }
                        }
                        Some(Ok(_)) => {}
                    }
                }
            }
        }
    }
    if was_cancelled {
        s.reconnect().await?;
        return Err(cancelled());
    }
    plan.ok_or_else(|| {
        DriverError::new(
            ErrorKind::Unsupported,
            "この文の実行計画は、SQL Server から返りませんでした",
        )
    })
}
