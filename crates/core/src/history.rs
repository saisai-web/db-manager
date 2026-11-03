//! 実行履歴の記録。パスワードを含みうる文は保存しない。

use storage::{HistoryStatus, NewHistory};

use crate::core::Shared;
use crate::plan::Planned;
use crate::tab::Tab;
use crate::types::*;

impl Shared {
    pub(crate) fn record_history(&self, tab: &Tab, p: &Planned, outcome: &StatementOutcome) {
        // パスワードを含みうる文は、履歴に残さない
        if p.stmt.may_contain_password {
            return;
        }
        let (status, elapsed, affected) = match outcome {
            StatementOutcome::Completed {
                elapsed,
                affected_rows,
                ..
            } => (HistoryStatus::Ok, *elapsed, *affected_rows),
            StatementOutcome::Truncated { elapsed, rows, .. } => {
                (HistoryStatus::Ok, *elapsed, Some(*rows))
            }
            StatementOutcome::Failed { elapsed, .. }
            | StatementOutcome::TimedOut { elapsed, .. } => (HistoryStatus::Error, *elapsed, None),
            StatementOutcome::Cancelled { elapsed } => (HistoryStatus::Cancelled, *elapsed, None),
        };
        let entry = NewHistory {
            connection_id: tab.connection.id.clone(),
            sql_text: p.text.clone(),
            duration_ms: Some(elapsed.as_millis() as i64),
            affected_rows: affected.map(|n| n as i64),
            status,
            executed_at: None,
        };
        let keep = self.history_keep;
        // 履歴の保存に失敗しても、実行の結果には影響させない。送った順に保存される
        drop(self.storage.submit(move |s| {
            let id = s.add_history(&entry)?;
            if id % 100 == 0 {
                s.prune_history(&entry.connection_id, keep)?;
            }
            Ok(())
        }));
    }
}
