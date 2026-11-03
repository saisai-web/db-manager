//! 実行計画。推定は `EXPLAIN PLAN` と `PLAN_TABLE`、実測は文を実際に実行して
//! `V$SQL_PLAN_STATISTICS_ALL`(`STATISTICS_LEVEL = ALL` の間だけ集まる)から読む。
//! 行は JSON の配列にして core へ渡す([`PlanFormat::OracleJson`])。
//!
//! 明示的なトランザクションの外では、計画の読み取りまでを 1 つのトランザクションにして、終わったら
//! ROLLBACK する(`EXPLAIN PLAN` の行を残さない。読み取り専用モードでは、読み取り専用で始める)。

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use driver_api::{
    DriverError, ErrorKind, Flow, PlanFormat, Result, ResultColumn, RowSink, Session, Statement,
};
use serde_json::{Map, Value, json};

use crate::session::OracleSession;

const ORA_TABLE_OR_VIEW_DOES_NOT_EXIST: &str = "ORA-00942";

/// 計画の 1 行の列(`PLAN_TABLE` と `V$SQL_PLAN_STATISTICS_ALL` で共通)。数値の列は、JSON の数値にする。
const COLUMNS: [(&str, bool); 9] = [
    ("id", true),
    ("parent_id", true),
    ("operation", false),
    ("options", false),
    ("object_name", false),
    ("cost", true),
    ("cardinality", true),
    ("access_predicates", false),
    ("filter_predicates", false),
];

/// 実測の列(`V$SQL_PLAN_STATISTICS_ALL`)。JSON の項目名と、列の式。
const MEASURED: [(&str, &str); 3] = [
    ("starts", "last_starts"),
    ("output_rows", "last_output_rows"),
    ("elapsed_us", "last_elapsed_time"),
];

/// 行を文字列で集める。
#[derive(Default)]
struct Rows(Vec<Vec<Option<String>>>);

#[async_trait]
impl RowSink for Rows {
    fn columns(&mut self, _: &[ResultColumn]) -> Flow {
        Flow::Continue
    }

    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        self.0.push(
            cells
                .iter()
                .map(|c| c.map(|b| String::from_utf8_lossy(b).into_owned()))
                .collect(),
        );
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        Flow::Continue
    }
}

/// 結果を捨てる(実測のために、文を最後まで実行させるだけ)。
struct Drain;

#[async_trait]
impl RowSink for Drain {
    fn columns(&mut self, _: &[ResultColumn]) -> Flow {
        Flow::Continue
    }

    fn row(&mut self, _: &[Option<&[u8]>]) -> Flow {
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        Flow::Continue
    }
}

pub(crate) async fn explain(
    session: &mut OracleSession,
    sql: &str,
    analyze: bool,
) -> Result<(String, PlanFormat)> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if analyze {
        run(session, "ALTER SESSION SET STATISTICS_LEVEL = ALL").await?;
    }
    // 利用者の明示的なトランザクションの中では、その中で行う(終わらせない)
    let scoped = !session.worker.mode.in_transaction();
    // 推定は、文を実行しない。`EXPLAIN PLAN` は計画表に書くので、読み取り専用のトランザクションでは
    // 動かない(ORA-01456)。そのため、この間だけ、読み取り専用を外す
    let read_only = session.worker.mode.read_only();
    if !analyze {
        session.worker.mode.read_only.store(false, Ordering::SeqCst);
    }
    let outcome = within_transaction(session, scoped, read_only && analyze, sql, analyze).await;
    session
        .worker
        .mode
        .read_only
        .store(read_only, Ordering::SeqCst);
    if analyze {
        let _ = run(session, "ALTER SESSION SET STATISTICS_LEVEL = TYPICAL").await;
    }
    Ok((outcome?, PlanFormat::OracleJson))
}

async fn within_transaction(
    session: &mut OracleSession,
    scoped: bool,
    read_only: bool,
    sql: &str,
    analyze: bool,
) -> Result<String> {
    if scoped && read_only {
        run(session, "SET TRANSACTION READ ONLY").await?;
    }
    let rows = if analyze {
        measured(session, sql).await
    } else {
        estimated(session, sql).await
    };
    if scoped {
        let _ = run(session, "ROLLBACK").await;
    }
    to_json(rows?, analyze)
}

async fn estimated(session: &mut OracleSession, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    // `PLAN_TABLE` はセッションごとの一時表。それでも、取り違えないよう、文ごとに別の ID を使う
    let id = format!("sqltool_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    run(
        session,
        &format!("EXPLAIN PLAN SET STATEMENT_ID = '{id}' FOR {sql}"),
    )
    .await?;
    let columns: Vec<&str> = COLUMNS.iter().map(|c| c.0).collect();
    collect(
        session,
        &format!(
            "SELECT {} FROM plan_table WHERE statement_id = '{id}' ORDER BY id",
            columns.join(", ")
        ),
    )
    .await
    .map_err(|e| missing_privilege(e, "PLAN_TABLE"))
}

async fn measured(session: &mut OracleSession, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
    session.execute(&Statement::new(sql), &mut Drain).await?;
    let columns: Vec<String> = COLUMNS
        .iter()
        .map(|c| c.0.to_string())
        .chain(MEASURED.iter().map(|m| m.1.to_string()))
        .collect();
    // `V$SESSION.PREV_SQL_ID` は、この問い合わせの 1 つ前、つまり利用者の文
    collect(
        session,
        &format!(
            "SELECT {} FROM v$sql_plan_statistics_all \
             WHERE (sql_id, child_number) = (SELECT prev_sql_id, prev_child_number FROM v$session \
             WHERE sid = SYS_CONTEXT('USERENV', 'SID')) ORDER BY id",
            columns.join(", ")
        ),
    )
    .await
    .map_err(|e| missing_privilege(e, "V$SQL_PLAN_STATISTICS_ALL と V$SESSION"))
}

async fn run(session: &mut OracleSession, sql: &str) -> Result<()> {
    session.execute(&Statement::new(sql), &mut Drain).await?;
    Ok(())
}

async fn collect(session: &mut OracleSession, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
    let mut rows = Rows::default();
    session.execute(&Statement::new(sql), &mut rows).await?;
    Ok(rows.0)
}

fn missing_privilege(mut e: DriverError, what: &str) -> DriverError {
    if e.code.as_deref() == Some(ORA_TABLE_OR_VIEW_DOES_NOT_EXIST) {
        e.message = format!(
            "{what} を読めません。実行計画には、これらを SELECT できる権限(SELECT_CATALOG_ROLE など)が要ります"
        );
    }
    e
}

/// 各行を、`PlanFormat::OracleJson` の形(項目名つきのオブジェクト)にする。
fn to_json(rows: Vec<Vec<Option<String>>>, analyze: bool) -> Result<String> {
    if rows.is_empty() {
        return Err(DriverError::new(ErrorKind::Other, "実行計画が空です"));
    }
    let names: Vec<(&str, bool)> = COLUMNS
        .iter()
        .copied()
        .chain(MEASURED.iter().map(|m| (m.0, true)))
        .take(COLUMNS.len() + if analyze { MEASURED.len() } else { 0 })
        .collect();
    let plan: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut o = Map::new();
            for ((name, numeric), cell) in names.iter().zip(row) {
                let v = match cell {
                    None => Value::Null,
                    Some(s) if *numeric => {
                        s.trim().parse::<f64>().map_or(Value::Null, |n| json!(n))
                    }
                    Some(s) => Value::String(s.clone()),
                };
                o.insert((*name).to_string(), v);
            }
            Value::Object(o)
        })
        .collect();
    Ok(Value::Array(plan).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_named_json_objects() {
        let rows = vec![vec![
            Some("0".to_string()),
            None,
            Some("SELECT STATEMENT".to_string()),
            None,
            None,
            Some("2".to_string()),
            Some("3".to_string()),
            None,
            Some("\"V\"='b'".to_string()),
        ]];
        let json = to_json(rows, false).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["id"], 0.0);
        assert!(v[0]["parent_id"].is_null());
        assert_eq!(v[0]["operation"], "SELECT STATEMENT");
        assert_eq!(v[0]["filter_predicates"], "\"V\"='b'");
        assert!(v[0].get("elapsed_us").is_none());
    }

    #[test]
    fn measured_rows_carry_the_run_statistics() {
        let mut row: Vec<Option<String>> = vec![Some("1".into()), Some("0".into())];
        row.extend((0..7).map(|_| None));
        row.extend(["3", "10", "1500"].map(|s| Some(s.to_string())));
        let v: Value = serde_json::from_str(&to_json(vec![row], true).unwrap()).unwrap();
        assert_eq!(v[0]["starts"], 3.0);
        assert_eq!(v[0]["output_rows"], 10.0);
        assert_eq!(v[0]["elapsed_us"], 1500.0);
    }

    #[test]
    fn an_empty_plan_is_an_error() {
        assert!(to_json(Vec::new(), false).is_err());
    }
}
