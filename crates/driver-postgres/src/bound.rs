//! バインド値つきの文の実行(拡張プロトコル)。テーブル編集の INSERT / UPDATE / DELETE に使う。
//!
//! 値は text として送り、SQL 側で列の型へ変換する(`Dialect::bind_text`)。結果セットは返さない。

use driver_api::{ExecSummary, Result, Statement};
use tokio_postgres::types::ToSql;

use crate::error::map_error;
use crate::session::PgSession;

impl PgSession {
    pub(crate) async fn execute_bound(&mut self, stmt: &Statement) -> Result<ExecSummary> {
        let prepared = self
            .client
            .prepare(&stmt.sql)
            .await
            .map_err(|e| map_error(&e))?;
        let params: Vec<&(dyn ToSql + Sync)> = stmt
            .params
            .iter()
            .map(|p| p as &(dyn ToSql + Sync))
            .collect();
        let affected = self
            .client
            .execute(&prepared, &params)
            .await
            .map_err(|e| map_error(&e))?;
        Ok(ExecSummary {
            rows_received: 0,
            affected_rows: Some(affected),
            stopped_early: false,
        })
    }
}
