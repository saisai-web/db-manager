//! 実行と結果のイベントを、モデルに適用する。

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, RopeExt as _};
use gpui_kit::{Context, Window};
use sqltool_core::{Event, ExecutionStatus, ResultId, StatementOutcome, TabId, TruncateReason};

use super::AppModel;
use super::status::StatusLine;
use crate::ui::ResultView;

pub fn fmt_elapsed(d: Duration) -> String {
    if d < Duration::from_millis(1) {
        format!("{} µs", d.as_micros())
    } else if d < Duration::from_secs(1) {
        format!("{} ms", d.as_millis())
    } else {
        format!("{:.2} 秒", d.as_secs_f64())
    }
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl AppModel {
    pub(super) fn apply_exec(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::ExecutionStarted { tab, .. } => self.on_execution_started(tab),
            Event::StatementStarted { tab, index, .. } => {
                self.set_status(
                    tab,
                    StatusLine::info(format!("実行中… ({}文目)", index + 1)),
                );
            }
            Event::ResultOpened { tab, result } => {
                let core = self.core.clone();
                if let Some(t) = self.tab_mut(tab) {
                    let view = ResultView::new(&core, result.clone(), window, cx);
                    let table = view.table.clone();
                    if let Some(edit) = &view.edit {
                        // 入力欄で Enter を押したら、セルに適用する
                        cx.subscribe(&edit.input.clone(), move |m, _, ev: &InputEvent, cx| {
                            if matches!(ev, InputEvent::PressEnter { .. }) {
                                m.apply_cell_edit(tab, false, cx);
                            }
                        })
                        .detach();
                    }
                    t.results.push(view);
                    // セルの選択を受けて、編集バーに値を入れる
                    cx.subscribe_in(&table, window, move |m, _, ev, window, cx| {
                        m.on_table_event(tab, result.id(), ev, window, cx)
                    })
                    .detach();
                    t.active_result = t.results.len() - 1;
                }
            }
            Event::Progress { result, rows, .. } => {
                self.update_result(result, cx, |r| r.rows = rows);
                self.set_progress_text(result, rows);
            }
            Event::FetchPaused { result, rows } => {
                self.update_result(result, cx, |r| {
                    r.rows = rows;
                    r.paused = true;
                });
                for t in &mut self.tabs {
                    if t.results.iter().any(|r| r.handle.id() == result) {
                        t.status = StatusLine::warn(format!(
                            "{} 行で一時停止しています。続きを取得するか、打ち切ってください",
                            group_digits(rows)
                        ));
                    }
                }
            }
            Event::StatementFinished {
                tab,
                result,
                outcome,
                ..
            } => self.on_statement_finished(tab, result, outcome, window, cx),
            Event::ExecutionFinished { tab, status } => self.on_execution_finished(tab, status),
            Event::SortChanged { result } => self.update_result(result, cx, |_| {}),
            _ => {}
        }
    }

    fn on_execution_started(&mut self, tab: TabId) {
        // 前の実行の結果は閉じる。メモリと一時ファイルを手放すため、core にも知らせる
        let Some(t) = self.tab_mut(tab) else { return };
        let old: Vec<ResultId> = t.results.drain(..).map(|r| r.handle.id()).collect();
        t.active_result = 0;
        t.status = StatusLine::info("実行中…");
        for result in old {
            self.core
                .send(sqltool_core::Command::CloseResult { result });
        }
    }

    fn update_result(
        &mut self,
        id: ResultId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ResultView),
    ) {
        for t in &mut self.tabs {
            if let Some(r) = t.result_mut(id) {
                f(r);
                r.refresh(cx);
                return;
            }
        }
    }

    fn set_progress_text(&mut self, result: ResultId, rows: u64) {
        for t in &mut self.tabs {
            if t.results.iter().any(|r| r.handle.id() == result) {
                t.status = StatusLine::info(format!("受信中… {} 行", group_digits(rows)));
            }
        }
    }

    fn on_statement_finished(
        &mut self,
        tab: TabId,
        result: Option<ResultId>,
        outcome: StatementOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line = match &outcome {
            StatementOutcome::Completed {
                rows,
                affected_rows,
                elapsed,
                has_result_set,
            } => {
                let t = fmt_elapsed(*elapsed);
                if *has_result_set {
                    StatusLine::ok(format!("{} 行 ({t})", group_digits(*rows)))
                } else {
                    StatusLine::ok(format!(
                        "完了。影響行数 {} ({t})",
                        affected_rows.unwrap_or(0)
                    ))
                }
            }
            StatementOutcome::Truncated { rows, reason, .. } => {
                let why = match reason {
                    TruncateReason::UserStopped => "打ち切りました",
                    TruncateReason::MemoryLimit => "結果のメモリ上限に達したため打ち切りました",
                };
                StatusLine::warn(format!(
                    "{} 行で{why}。並べ替えはできません",
                    group_digits(*rows)
                ))
            }
            StatementOutcome::Failed { error, .. } => {
                let code = error
                    .code
                    .as_ref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default();
                StatusLine::error(format!("{}{code}", error.message))
            }
            StatementOutcome::Cancelled { .. } => StatusLine::warn("キャンセルしました"),
            StatementOutcome::TimedOut { limit, .. } => {
                StatusLine::error(format!("{} 経過したため中止しました", fmt_elapsed(*limit)))
            }
        };
        if let StatementOutcome::Failed {
            editor_offset: Some(off),
            ..
        } = &outcome
        {
            self.move_cursor_to(tab, *off, window, cx);
        }
        if let Some(id) = result {
            self.update_result(id, cx, |r| {
                r.paused = false;
                r.rows = r.handle.row_count();
                r.outcome = Some(outcome);
            });
        }
        self.set_status(tab, line);
    }

    /// エラー位置が分かる場合は、エディタ上のその位置へカーソルを動かす。
    fn move_cursor_to(
        &self,
        tab: TabId,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.tab(tab) else { return };
        t.editor().update(cx, |e, cx| {
            let pos = e.text().offset_to_position(offset);
            e.set_cursor_position(pos, window, cx);
        });
    }

    fn on_execution_finished(&mut self, tab: TabId, status: ExecutionStatus) {
        let line = match status {
            ExecutionStatus::RefusedReadOnly { .. } => Some(StatusLine::error(
                "読み取り専用モードのため、書き込みを含む文は実行しませんでした",
            )),
            ExecutionStatus::Declined => Some(StatusLine::warn("実行を取りやめました")),
            ExecutionStatus::NothingToRun => Some(StatusLine::info("実行する文がありません")),
            _ => None,
        };
        if let Some(line) = line {
            self.set_status(tab, line);
        }
        if let Some(t) = self.tab_mut(tab) {
            t.pending_confirm = None;
        }
    }
}
