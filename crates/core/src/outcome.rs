//! 1 つの文の実行結果を、利用者に見せる結果と、実行全体の状態にする。
//! DB にも非同期処理にも依存しない純粋なロジックで、単体テストで網羅する。

use std::time::Duration;

use driver_api::{DriverError, ErrorKind, ExecSummary};

use crate::plan::{Planned, editor_offset};
use crate::types::{ExecutionStatus, StatementOutcome, TruncateReason, UserError};

/// 結果の受け取り側(シンク)が、実行中に見たこと。
pub(crate) struct SinkReport {
    /// 結果ストアに保持した行数。結果セットを返さない文では `None`
    pub held_rows: Option<u64>,
    /// 結果ストアへの追記が、上限以外の理由で失敗した
    pub store_failure: Option<String>,
    pub truncated: Option<TruncateReason>,
}

/// 実行の条件と経過。
pub(crate) struct RunFacts {
    pub elapsed: Duration,
    /// 利用者が停止を求めた
    pub cancel_requested: bool,
    /// タイムアウトでサーバーに停止を依頼した
    pub timed_out: bool,
    pub timeout: Option<Duration>,
}

pub(crate) struct Classified {
    pub outcome: StatementOutcome,
    /// 後続の文を実行せずに、実行全体を止める
    pub stop: bool,
    /// 接続が切れた
    pub disconnected: bool,
}

pub(crate) fn classify(
    result: Result<ExecSummary, DriverError>,
    sink: SinkReport,
    planned: &Planned,
    run: &RunFacts,
) -> Classified {
    let elapsed = run.elapsed;
    match result {
        Ok(summary) => {
            // 保持した行数を正とする。ドライバは、シンクが受け取らなかった行も数える
            let rows = sink.held_rows.unwrap_or(summary.rows_received);
            let (outcome, stop) = if let Some(message) = sink.store_failure {
                (
                    StatementOutcome::Failed {
                        error: UserError::new(message),
                        editor_offset: None,
                        elapsed,
                    },
                    true,
                )
            } else if let Some(reason) = sink.truncated {
                // 打ち切りはエラーではない。ただし、停止を求められていれば、後続も実行しない
                (
                    StatementOutcome::Truncated {
                        rows,
                        elapsed,
                        reason,
                    },
                    run.cancel_requested,
                )
            } else {
                (
                    StatementOutcome::Completed {
                        rows,
                        affected_rows: summary.affected_rows,
                        elapsed,
                        has_result_set: sink.held_rows.is_some(),
                    },
                    false,
                )
            };
            Classified {
                outcome,
                stop,
                disconnected: false,
            }
        }
        Err(e) => {
            let disconnected = e.kind == ErrorKind::Disconnected;
            // タイムアウトと利用者の停止は、サーバーからは同じ「キャンセル」として返る
            let outcome = if run.timed_out {
                StatementOutcome::TimedOut {
                    elapsed,
                    limit: run.timeout.unwrap_or_default(),
                }
            } else if run.cancel_requested || e.kind == ErrorKind::Cancelled {
                StatementOutcome::Cancelled { elapsed }
            } else {
                let editor_offset = e.position.map(|pos| editor_offset(planned, pos));
                StatementOutcome::Failed {
                    error: e.into(),
                    editor_offset,
                    elapsed,
                }
            };
            Classified {
                outcome,
                stop: true,
                disconnected,
            }
        }
    }
}

/// 文の結果が実行全体を止めたときの、実行全体の状態。
pub(crate) fn status_when_stopped(outcome: &StatementOutcome, index: usize) -> ExecutionStatus {
    match outcome {
        StatementOutcome::Failed { .. } => ExecutionStatus::Failed {
            statement_index: index,
        },
        StatementOutcome::TimedOut { .. } => ExecutionStatus::TimedOut {
            statement_index: index,
        },
        _ => ExecutionStatus::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ExecuteTarget, plan};
    use sqlscan::Rules;

    fn planned(sql: &str) -> Planned {
        plan(sql, &ExecuteTarget::All, &Rules::postgres()).remove(0)
    }

    fn facts() -> RunFacts {
        RunFacts {
            elapsed: Duration::from_millis(5),
            cancel_requested: false,
            timed_out: false,
            timeout: None,
        }
    }

    fn sink(held: Option<u64>) -> SinkReport {
        SinkReport {
            held_rows: held,
            store_failure: None,
            truncated: None,
        }
    }

    fn summary(rows: u64, affected: Option<u64>) -> ExecSummary {
        ExecSummary {
            rows_received: rows,
            affected_rows: affected,
            stopped_early: false,
        }
    }

    fn err(kind: ErrorKind) -> DriverError {
        DriverError::new(kind, "boom")
    }

    #[test]
    fn a_result_set_completes_with_the_held_row_count() {
        let c = classify(
            Ok(summary(10, Some(10))),
            sink(Some(10)),
            &planned("select 1"),
            &facts(),
        );
        assert!(!c.stop && !c.disconnected);
        assert!(matches!(
            c.outcome,
            StatementOutcome::Completed {
                rows: 10,
                affected_rows: Some(10),
                has_result_set: true,
                ..
            }
        ));
    }

    #[test]
    fn a_statement_without_a_result_set_reports_affected_rows() {
        let c = classify(
            Ok(summary(0, Some(3))),
            sink(None),
            &planned("update t set a = 1"),
            &facts(),
        );
        assert!(matches!(
            c.outcome,
            StatementOutcome::Completed {
                rows: 0,
                affected_rows: Some(3),
                has_result_set: false,
                ..
            }
        ));
    }

    #[test]
    fn held_rows_win_over_the_drivers_count() {
        // メモリ上限で 1 行を受け取らなかった場合、ドライバは 1 行多く数える
        let mut s = sink(Some(2000));
        s.truncated = Some(TruncateReason::MemoryLimit);
        let c = classify(Ok(summary(2001, None)), s, &planned("select 1"), &facts());
        assert!(matches!(
            c.outcome,
            StatementOutcome::Truncated {
                rows: 2000,
                reason: TruncateReason::MemoryLimit,
                ..
            }
        ));
    }

    #[test]
    fn truncation_continues_the_batch_unless_a_stop_was_requested() {
        let mut s = sink(Some(5));
        s.truncated = Some(TruncateReason::UserStopped);
        let c = classify(Ok(summary(5, None)), s, &planned("select 1"), &facts());
        assert!(!c.stop, "stopping one result does not end the batch");

        let mut s = sink(Some(5));
        s.truncated = Some(TruncateReason::UserStopped);
        let mut f = facts();
        f.cancel_requested = true;
        let c = classify(Ok(summary(5, None)), s, &planned("select 1"), &f);
        assert!(c.stop, "Cancel ends the batch");
        assert_eq!(
            status_when_stopped(&c.outcome, 0),
            ExecutionStatus::Cancelled
        );
    }

    #[test]
    fn a_store_failure_is_a_failed_statement() {
        let mut s = sink(Some(1));
        s.store_failure = Some("disk full".into());
        let c = classify(Ok(summary(1, None)), s, &planned("select 1"), &facts());
        assert!(c.stop);
        assert!(
            matches!(c.outcome, StatementOutcome::Failed { ref error, .. } if error.message == "disk full")
        );
        assert_eq!(
            status_when_stopped(&c.outcome, 2),
            ExecutionStatus::Failed { statement_index: 2 }
        );
    }

    #[test]
    fn errors_map_to_failed_with_the_editor_offset() {
        let p = planned("select '日本語', from t");
        let mut e = err(ErrorKind::Query);
        e.position = Some(p.text.chars().position(|c| c == 'f').unwrap());
        e.code = Some("42601".into());
        let c = classify(Err(e), sink(None), &p, &facts());
        assert!(c.stop && !c.disconnected);
        let StatementOutcome::Failed {
            error,
            editor_offset,
            ..
        } = c.outcome
        else {
            panic!()
        };
        assert_eq!(error.code.as_deref(), Some("42601"));
        assert_eq!(
            &"select '日本語', from t"[editor_offset.unwrap()..][..4],
            "from"
        );
    }

    #[test]
    fn timeout_and_user_cancel_are_told_apart_even_though_the_server_says_cancelled() {
        let mut f = facts();
        f.timed_out = true;
        f.timeout = Some(Duration::from_secs(3));
        let c = classify(
            Err(err(ErrorKind::Cancelled)),
            sink(None),
            &planned("select 1"),
            &f,
        );
        assert!(
            matches!(c.outcome, StatementOutcome::TimedOut { limit, .. } if limit == Duration::from_secs(3))
        );
        assert_eq!(
            status_when_stopped(&c.outcome, 1),
            ExecutionStatus::TimedOut { statement_index: 1 }
        );

        let c = classify(
            Err(err(ErrorKind::Cancelled)),
            sink(None),
            &planned("select 1"),
            &facts(),
        );
        assert!(matches!(c.outcome, StatementOutcome::Cancelled { .. }));

        // 利用者の停止は、サーバーが別のエラーで返しても、キャンセルとして扱う
        let mut f = facts();
        f.cancel_requested = true;
        let c = classify(
            Err(err(ErrorKind::Query)),
            sink(None),
            &planned("select 1"),
            &f,
        );
        assert!(matches!(c.outcome, StatementOutcome::Cancelled { .. }));
    }

    #[test]
    fn a_dropped_connection_is_flagged() {
        let c = classify(
            Err(err(ErrorKind::Disconnected)),
            sink(None),
            &planned("select 1"),
            &facts(),
        );
        assert!(c.stop && c.disconnected);
        assert!(
            matches!(c.outcome, StatementOutcome::Failed { ref error, .. } if error.kind == ErrorKind::Disconnected)
        );
    }
}
