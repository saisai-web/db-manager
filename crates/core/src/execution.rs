//! 実行の入口(文の決定、安全確認、利用者への確認)と、キャンセル、取得の制御。

use std::sync::Arc;
use std::time::Duration;

use crate::edit::EditAccess;
use crate::plan::{ExecuteTarget, assess, plan};
use crate::rules::scan_rules;
use crate::sink::FetchCommand;
use crate::tab::Tab;
use tokio::sync::oneshot;

use crate::core::{Res, Shared, lock};
use crate::types::*;

impl Shared {
    pub(crate) async fn execute(
        self: &Arc<Self>,
        tab_id: TabId,
        sql: String,
        target: ExecuteTarget,
        options: ExecuteOptions,
        access: EditAccess,
    ) -> Res {
        let tab = self.tab(tab_id)?;
        {
            let mut rt = tab.rt();
            if rt.run != RunState::Idle {
                return Err(UserError::new("実行中のため、新しい実行はできません"));
            }
            rt.run = RunState::Running;
            rt.cancel_requested = false;
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Running,
        });

        let result = self.execute_inner(&tab, sql, target, options, access).await;

        // 完了の通知を受けた側が、すぐ次の実行を送れるように、先にタブを空きに戻す。
        // 通知の順序(完了 → 空き)が入れ替わらないよう、状態のロックを持ったまま送る
        let mut rt = tab.rt();
        rt.run = RunState::Idle;
        rt.cancel_requested = false;
        rt.active_result = None;
        rt.pending_confirm = None;
        if let Ok(status) = &result {
            self.emit(Event::ExecutionFinished {
                tab: tab_id,
                status: *status,
            });
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Idle,
        });
        drop(rt);
        result.map(|_| ())
    }

    pub(crate) async fn execute_inner(
        self: &Arc<Self>,
        tab: &Arc<Tab>,
        sql: String,
        target: ExecuteTarget,
        options: ExecuteOptions,
        access: EditAccess,
    ) -> Res<ExecutionStatus> {
        let rules = scan_rules(tab.driver.dialect().lexical_features());
        let planned = plan(&sql, &target, &rules);
        if planned.is_empty() {
            return Ok(ExecutionStatus::NothingToRun);
        }

        let read_only = tab.rt().read_only;
        let assessment = assess(&planned, read_only);
        if let Some(r) = assessment.refusal {
            return Ok(ExecutionStatus::RefusedReadOnly {
                statement_index: r.statement_index,
                editor_offset: r.offset,
            });
        }

        if !assessment.confirmations.is_empty() {
            let id = ConfirmId(self.id());
            let (tx, rx) = oneshot::channel();
            lock(&self.confirmations).insert(id, tx);
            {
                let mut rt = tab.rt();
                rt.pending_confirm = Some(id);
                rt.run = RunState::WaitingConfirmation;
            }
            self.emit(Event::RunStateChanged {
                tab: tab.id,
                state: RunState::WaitingConfirmation,
            });
            self.emit(Event::ConfirmationRequired {
                id,
                tab: tab.id,
                items: assessment.confirmations,
            });
            let accepted = rx.await.unwrap_or(false);
            let cancelled = {
                let mut rt = tab.rt();
                rt.pending_confirm = None;
                if !rt.cancel_requested {
                    rt.run = RunState::Running;
                }
                rt.cancel_requested
            };
            if !accepted || cancelled {
                return Ok(ExecutionStatus::Declined);
            }
            self.emit(Event::RunStateChanged {
                tab: tab.id,
                state: RunState::Running,
            });
        }

        Ok(self.run_batch(tab, planned, &options, &access).await)
    }

    pub(crate) fn cancel(self: &Arc<Self>, tab_id: TabId) -> Res {
        let tab = self.tab(tab_id)?;
        let mut rt = tab.rt();
        match rt.run {
            RunState::Idle | RunState::Cancelling => {}
            RunState::WaitingConfirmation => {
                rt.cancel_requested = true;
                if let Some(id) = rt.pending_confirm.take()
                    && let Some(tx) = lock(&self.confirmations).remove(&id)
                {
                    let _ = tx.send(false);
                }
            }
            RunState::Running | RunState::Paused { .. } => {
                let paused = matches!(rt.run, RunState::Paused { .. });
                rt.cancel_requested = true;
                rt.run = RunState::Cancelling;
                let active = rt.active_result;
                drop(rt);
                self.emit(Event::RunStateChanged {
                    tab: tab_id,
                    state: RunState::Cancelling,
                });
                if paused {
                    if let Some(tx) =
                        active.and_then(|r| lock(&self.fetch_controls).get(&r).cloned())
                    {
                        let _ = tx.send(FetchCommand::Stop);
                    }
                } else {
                    // サーバー側の停止完了は待たない。UI はすぐに操作できる
                    self.spawn_cancel(&tab);
                }
            }
        }
        Ok(())
    }

    /// 実行中の文を止めるよう、サーバーに依頼する。
    ///
    /// 要求が、文がサーバーに届く前に着くと、サーバーは「止める文がない」として無視する。
    /// 実行を始めた直後の停止が失われないよう、文が終わるまで再送する。再送は、その文
    /// (世代)に限る。終わった後に届いて、次の文を止めてしまわないため。
    pub(crate) fn spawn_cancel(self: &Arc<Self>, tab: &Arc<Tab>) {
        const RESEND: Duration = Duration::from_millis(300);
        const POLL: Duration = Duration::from_millis(20);
        // 実行中の文、まだ始まっていなければ次に始まる文の世代
        let generation = {
            let rt = tab.rt();
            rt.statement_seq + u64::from(!rt.in_statement)
        };
        let canceller = tab.canceller();
        let tab = Arc::clone(tab);
        let events = self.events.clone();
        self.rt.spawn(async move {
            loop {
                let (running, started, current) = {
                    let rt = tab.rt();
                    (rt.run != RunState::Idle, rt.in_statement, rt.statement_seq)
                };
                if !running || current > generation {
                    return;
                }
                if started && current == generation {
                    if let Err(e) = canceller.cancel().await {
                        let _ = events.send(Event::CommandFailed {
                            command: "Cancel",
                            tab: Some(tab.id),
                            error: e.into(),
                        });
                        return; // 停止の要求自体が通らない。繰り返さない
                    }
                    tokio::time::sleep(RESEND).await;
                } else {
                    tokio::time::sleep(POLL).await;
                }
            }
        });
    }

    pub(crate) fn fetch_control(&self, result: ResultId, command: FetchCommand) -> Res {
        let tx = lock(&self.fetch_controls).get(&result).cloned();
        match tx {
            Some(tx) if tx.send(command).is_ok() => Ok(()),
            _ => Err(UserError::new("この結果は取得を一時停止していません")),
        }
    }
}
