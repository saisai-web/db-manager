//! 文を順に実行するループと、1 つの文の実行。
//! 結果の判定は `outcome`(純粋関数)に任せ、ここは非同期の進行と後始末だけを持つ。

use std::sync::Arc;
use std::time::{Duration, Instant};

use driver_api::{ErrorKind, Session, Statement};
use result_store::StoreConfig;
use sqlscan::StatementKind;
use tokio::sync::mpsc::unbounded_channel;

use crate::core::{Shared, lock};
use crate::edit::EditAccess;
use crate::outcome::{RunFacts, classify, status_when_stopped};
use crate::plan::{Planned, TxnState};
use crate::result::ResultHandle;
use crate::sink::{PauseHook, ResultSink, SinkParams};
use crate::tab::Tab;
use crate::types::*;

/// 1 つの文を実行するときの、実行全体から引き継ぐ条件。
#[derive(Clone, Copy)]
struct StatementCtx<'a> {
    index: usize,
    fetch_limit: u64,
    options: &'a ExecuteOptions,
    access: &'a EditAccess,
}

/// 1 つの文を実行し終えたときの、次の動きの指示。
struct StatementDone {
    /// `Some` なら、後続の文を実行せずに、この状態で実行を終える
    stopped: Option<ExecutionStatus>,
    disconnected: bool,
}

fn not_connected() -> UserError {
    UserError {
        kind: ErrorKind::Disconnected,
        ..UserError::new("接続が切れています。再接続してください")
    }
}

fn resolve_fetch_limit(limit: FetchLimit, default: u64) -> u64 {
    match limit {
        FetchLimit::Default | FetchLimit::Rows(0) => default,
        FetchLimit::Rows(n) => n,
        FetchLimit::Unlimited => u64::MAX,
    }
}

impl Shared {
    pub(crate) async fn run_batch(
        self: &Arc<Self>,
        tab: &Arc<Tab>,
        planned: Vec<Planned>,
        options: &ExecuteOptions,
        access: &EditAccess,
    ) {
        let fetch_limit = resolve_fetch_limit(options.fetch_limit, self.default_fetch_limit);
        self.emit(Event::ExecutionStarted {
            tab: tab.id,
            statements: planned.len(),
        });

        let mut guard = tab.session.lock().await;
        let mut status = ExecutionStatus::Completed;
        for (index, p) in planned.iter().enumerate() {
            if tab.rt().cancel_requested {
                // 文を始める前に停止が求められた。何も送らない
                status = ExecutionStatus::Cancelled;
                break;
            }
            let Some(session) = guard.as_mut() else {
                self.emit(Event::CommandFailed {
                    command: "Execute",
                    tab: Some(tab.id),
                    error: not_connected(),
                });
                status = ExecutionStatus::Failed {
                    statement_index: index,
                };
                break;
            };
            let done = self
                .run_statement(
                    tab,
                    session.as_mut(),
                    p,
                    &StatementCtx {
                        index,
                        fetch_limit,
                        options,
                        access,
                    },
                )
                .await;
            if done.disconnected {
                guard.take();
                self.on_disconnected(tab);
            }
            if let Some(stopped) = done.stopped {
                status = stopped;
                break;
            }
        }
        drop(guard);
        self.emit(Event::ExecutionFinished {
            tab: tab.id,
            status,
        });
    }

    /// 文を 1 つ実行し、結果を判定して、後始末(トランザクション、履歴、スキーマ)をする。
    async fn run_statement(
        self: &Arc<Self>,
        tab: &Arc<Tab>,
        session: &mut dyn Session,
        p: &Planned,
        ctx: &StatementCtx<'_>,
    ) -> StatementDone {
        let StatementCtx {
            index,
            fetch_limit,
            options,
            access,
        } = *ctx;
        self.emit(Event::StatementStarted {
            tab: tab.id,
            index,
            range: p.range.clone(),
        });
        let (mut sink, result_id) = self.make_sink(tab, index, p, fetch_limit, access);

        {
            let mut rt = tab.rt();
            rt.statement_seq += 1;
            rt.in_statement = true;
        }
        let started = Instant::now();
        let (result, timed_out) = self
            .execute_one(tab, session, p, &mut sink, options.timeout)
            .await;
        let elapsed = started.elapsed();
        tab.rt().in_statement = false;
        lock(&self.fetch_controls).remove(&result_id);

        let cancel_requested = tab.rt().cancel_requested;
        sink.finish(result.is_ok());
        let classified = classify(
            result,
            sink.report(),
            p,
            &RunFacts {
                elapsed,
                cancel_requested,
                timed_out,
                timeout: options.timeout,
            },
        );

        self.update_txn(tab, p, &classified.outcome);
        self.record_history(tab, p, &classified.outcome);
        self.invalidate_schema_after_ddl(tab, p, &classified.outcome)
            .await;

        let stopped = classified
            .stop
            .then(|| status_when_stopped(&classified.outcome, index));
        self.emit(Event::StatementFinished {
            tab: tab.id,
            index,
            result: sink.result_id(),
            outcome: classified.outcome,
        });
        StatementDone {
            stopped,
            disconnected: classified.disconnected,
        }
    }

    /// 結果の受け取り側を作り、一時停止の解除先(取得の制御)を登録する。
    fn make_sink(
        self: &Arc<Self>,
        tab: &Arc<Tab>,
        index: usize,
        p: &Planned,
        fetch_limit: u64,
        access: &EditAccess,
    ) -> (ResultSink, ResultId) {
        let result_id = ResultId(self.id());
        let (control_tx, control_rx) = unbounded_channel();
        lock(&self.fetch_controls).insert(result_id, control_tx);
        tab.rt().active_result = Some(result_id);

        let mut store_config = StoreConfig::new(self.spill_dir.clone());
        store_config.memory_limit = self.memory_limit;
        let sh = Arc::clone(self);
        let sink = ResultSink::new(SinkParams {
            tab: tab.id,
            statement_index: index,
            sql: p.text.clone(),
            result_id,
            store_config,
            events: self.events.clone(),
            fetch_limit,
            control: control_rx,
            register: Arc::new(move |h: ResultHandle| {
                sh.results().insert(h.id(), h);
            }),
            on_pause: self.pause_hook(tab),
            access: access.clone(),
        });
        (sink, result_id)
    }

    /// DDL が成功したら、スキーマのキャッシュが古くなったと知らせる。
    async fn invalidate_schema_after_ddl(
        &self,
        tab: &Tab,
        p: &Planned,
        outcome: &StatementOutcome,
    ) {
        let is_ddl = matches!(
            p.stmt.kind,
            StatementKind::Create | StatementKind::Alter | StatementKind::Drop
        );
        if !(is_ddl && matches!(outcome, StatementOutcome::Completed { .. })) {
            return;
        }
        let connection_id = tab.connection.id.clone();
        let cache = lock(&self.schemas).get(&connection_id).cloned();
        if let Some(cache) = cache {
            cache.invalidate().await;
        }
        self.emit(Event::SchemaInvalidated { connection_id });
    }

    /// 接続が切れたタブの後始末。トランザクションは接続と一緒に失われる。
    pub(crate) fn on_disconnected(&self, tab: &Tab) {
        let had_txn = std::mem::replace(&mut tab.rt().txn, TxnState::Idle).is_pending();
        if had_txn {
            self.emit(Event::TransactionChanged {
                tab: tab.id,
                state: TxnState::Idle,
            });
        }
        self.emit(Event::TabDisconnected {
            tab: tab.id,
            error: UserError {
                kind: ErrorKind::Disconnected,
                ..UserError::new("DB との接続が切れました")
            },
        });
    }

    /// 1 つの文を実行する。タイムアウトしたら、サーバーに停止を依頼したうえで、
    /// 実行の終わりまで待つ(途中で捨てると、接続に前のクエリが残るため)。
    pub(crate) async fn execute_one(
        self: &Arc<Self>,
        tab: &Arc<Tab>,
        session: &mut dyn Session,
        p: &Planned,
        sink: &mut ResultSink,
        timeout: Option<Duration>,
    ) -> (driver_api::Result<driver_api::ExecSummary>, bool) {
        let stmt = Statement::new(p.text.clone());
        let fut = session.execute(&stmt, sink);
        tokio::pin!(fut);
        let timer = async {
            match timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timer);
        let mut timed_out = false;
        let result = loop {
            tokio::select! {
                r = &mut fut => break r,
                _ = &mut timer, if !timed_out => {
                    timed_out = true;
                    self.spawn_cancel(tab);
                }
            }
        };
        (result, timed_out)
    }

    pub(crate) fn pause_hook(self: &Arc<Self>, tab: &Arc<Tab>) -> PauseHook {
        let sh = Arc::clone(self);
        let tab = Arc::clone(tab);
        Arc::new(move |paused: Option<ResultId>| {
            let state = {
                let mut rt = tab.rt();
                if rt.cancel_requested {
                    return; // 停止中の表示を、一時停止や再開で上書きしない
                }
                rt.run = match paused {
                    Some(result) => RunState::Paused { result },
                    None => RunState::Running,
                };
                rt.run
            };
            sh.emit(Event::RunStateChanged { tab: tab.id, state });
        })
    }

    pub(crate) fn update_txn(&self, tab: &Tab, p: &Planned, outcome: &StatementOutcome) {
        let ok = matches!(
            outcome,
            StatementOutcome::Completed { .. } | StatementOutcome::Truncated { .. }
        );
        let (before, after) = {
            let mut rt = tab.rt();
            let before = rt.txn;
            rt.txn = if ok {
                before.after_success(&p.stmt)
            } else {
                before.after_error()
            };
            (before, rt.txn)
        };
        if before != after {
            self.emit(Event::TransactionChanged {
                tab: tab.id,
                state: after,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_limit_resolution() {
        assert_eq!(resolve_fetch_limit(FetchLimit::Default, 100_000), 100_000);
        assert_eq!(resolve_fetch_limit(FetchLimit::Rows(0), 100_000), 100_000);
        assert_eq!(resolve_fetch_limit(FetchLimit::Rows(5), 100_000), 5);
        assert_eq!(
            resolve_fetch_limit(FetchLimit::Unlimited, 100_000),
            u64::MAX
        );
    }
}
