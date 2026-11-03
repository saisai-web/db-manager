//! 実行計画の表示。

use gpui_kit::Context;
use sqltool_core::{Command, Event, Plan, TabId};

use super::AppModel;
use super::actions::editor_target;
use super::status::StatusLine;

pub struct ExplainState {
    pub sql: String,
    pub analyze: bool,
    pub plan: Plan,
}

impl AppModel {
    /// カーソル位置の文(選択範囲があればその範囲の最初の文)の実行計画を求める。
    pub fn explain(&mut self, tab: TabId, analyze: bool, cx: &mut Context<Self>) {
        let Some(t) = self.tab(tab) else { return };
        let (sql, target) = t.editor.read_with(cx, |e, _| editor_target(e, false));
        self.set_status(tab, StatusLine::info("実行計画を取得中…"));
        self.core.send(Command::Explain {
            tab,
            sql,
            target,
            analyze,
        });
    }

    pub fn close_explain(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(tab) {
            t.explain = None;
        }
        cx.notify();
    }

    pub(super) fn apply_explain_event(&mut self, event: Event, cx: &mut Context<Self>) {
        if let Event::ExplainReady {
            tab,
            sql,
            analyze,
            plan,
        } = event
            && let Some(t) = self.tab_mut(tab)
        {
            t.explain = Some(ExplainState { sql, analyze, plan });
            t.status = StatusLine::ok("実行計画を取得しました");
        }
        cx.notify();
    }
}
