//! 実行履歴とスニペット。

use gpui_kit::component::input::{InputContent, InputState};
use gpui_kit::{AppContext as _, Context, Window};
use sqltool_core::{Command, Event, HistoryQuery, TabId};

use super::AppModel;
use super::status::StatusLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    History,
    Snippets,
}

/// パネルに出す履歴の件数
const HISTORY_LIMIT: usize = 100;

impl AppModel {
    pub fn open_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if self.library_input.is_none() {
            self.library_input = Some(cx.new(|cx| InputState::new(window, cx)));
        }
        self.panel = Some(panel);
        match panel {
            Panel::History => self.search_history(cx),
            Panel::Snippets => self.core.send(Command::ListSnippets),
        }
        cx.notify();
    }

    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        cx.notify();
    }

    /// 入力欄の文字を含む履歴を求める(空なら、最近のもの)。選択中の接続のものだけ。
    pub fn search_history(&mut self, cx: &mut Context<Self>) {
        let text = self
            .library_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .filter(|s| !s.is_empty());
        self.core.send(Command::ListHistory(HistoryQuery {
            connection_id: self.active_tab().map(|t| t.connection.id.clone()),
            contains: text,
            limit: HISTORY_LIMIT,
        }));
    }

    /// 履歴やスニペットの SQL を、エディタの末尾に足す(書きかけを消さない)。
    pub fn insert_into_editor(
        &mut self,
        tab: TabId,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.tab(tab) else { return };
        t.editor.update(cx, |e, cx| {
            let current = e.value().to_string();
            let joined = if current.trim().is_empty() {
                sql.to_string()
            } else {
                format!("{}\n\n{sql}", current.trim_end())
            };
            e.set_value(InputContent::from(joined), window, cx);
        });
        self.panel = None;
        cx.notify();
    }

    /// エディタの SQL(選択範囲があればその範囲)を、入力欄の名前でスニペットに保存する。
    pub fn save_snippet_from_editor(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(t) = self.tab(tab) else { return };
        let sql = t.editor.read_with(cx, |e, _| {
            let range = e.selected_range();
            let text = e.value().to_string();
            if range.is_empty() {
                text
            } else {
                text[range].to_string()
            }
        });
        let name = self
            .library_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if sql.trim().is_empty() {
            self.set_status(tab, StatusLine::warn("保存する SQL がありません"));
            return;
        }
        self.core.send(Command::SaveSnippet {
            id: None,
            name,
            sql,
        });
    }

    pub fn delete_snippet(&mut self, id: String) {
        self.core.send(Command::DeleteSnippet { id });
    }

    pub(super) fn apply_library_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::History(list) => self.history = list,
            Event::Snippets(list) => self.snippets = list,
            _ => {}
        }
        cx.notify();
    }
}

/// 一覧の 1 行に出す、SQL の要約(空白を詰め、長ければ切る)。
pub fn preview(sql: &str) -> String {
    sqltool_core::summarize(sql)
}
