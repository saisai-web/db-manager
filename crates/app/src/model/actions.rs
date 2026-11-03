//! 利用者の操作を、core のコマンドにする。

use gpui_kit::Context;
use sqltool_core::{
    Command, ConnectionId, ConnectionInput, ExecuteOptions, ExecuteTarget, FetchMore, ResultId,
    TabId, TableRef,
};

use super::AppModel;
use super::schema::KEY_SEP;

/// エディタの内容と、何を実行するか(全文、選択範囲、カーソル位置の文)。
pub(super) fn editor_target(
    e: &gpui_kit::component::input::EditorState,
    all: bool,
) -> (String, ExecuteTarget) {
    let sel = e.selected_range();
    let target = if all {
        ExecuteTarget::All
    } else if !sel.is_empty() {
        ExecuteTarget::Selection {
            start: sel.start,
            end: sel.end,
        }
    } else {
        ExecuteTarget::Cursor { offset: e.cursor() }
    };
    (e.value().to_string(), target)
}

impl AppModel {
    // ---- 接続 ----

    pub fn save_connection(&mut self, input: ConnectionInput) {
        self.core.send(Command::SaveConnection(input));
    }

    pub fn test_connection(&mut self, input: ConnectionInput, cx: &mut Context<Self>) {
        self.test_result = None;
        self.last_attempt = Some(super::Retry::Test);
        self.core.send(Command::TestConnection(input));
        cx.notify();
    }

    pub fn delete_connection(&mut self, id: ConnectionId) {
        self.core.send(Command::DeleteConnection { id });
    }

    pub fn open_tab(&mut self, connection_id: ConnectionId) {
        self.last_attempt = Some(super::Retry::Open(connection_id.clone()));
        self.core.send(Command::OpenTab { connection_id });
    }

    pub fn close_tab(&mut self, tab: TabId, force: bool) {
        self.core.send(Command::CloseTab { tab, force });
    }

    pub fn reconnect(&mut self, tab: TabId) {
        self.core.send(Command::Reconnect { tab });
    }

    pub fn activate(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.active = Some(tab);
        cx.notify();
    }

    pub fn set_read_only(&mut self, tab: TabId, read_only: bool) {
        self.core.send(Command::SetReadOnly { tab, read_only });
    }

    // ---- 実行 ----

    /// `all` なら全文、そうでなければ選択範囲(なければカーソル位置の文)を実行する。
    pub fn run(&mut self, tab: TabId, all: bool, cx: &mut Context<Self>) {
        let Some(t) = self.tab(tab) else { return };
        if !t.is_idle() {
            return;
        }
        let (sql, target) = t.editor.read_with(cx, |e, _| editor_target(e, all));
        self.core.send(Command::Execute {
            tab,
            sql,
            target,
            options: ExecuteOptions::default(),
        });
    }

    pub fn cancel(&mut self, tab: TabId) {
        self.core.send(Command::Cancel { tab });
    }

    pub fn answer_confirmation(&mut self, tab: TabId, accept: bool, cx: &mut Context<Self>) {
        let Some(t) = self.tab_mut(tab) else { return };
        if let Some(p) = t.pending_confirm.take() {
            self.core.send(Command::Confirm { id: p.id, accept });
        }
        cx.notify();
    }

    pub fn fetch_more(&mut self, result: ResultId, rest: bool) {
        let more = if rest {
            FetchMore::Rest
        } else {
            FetchMore::NextBatch
        };
        self.core.send(Command::FetchMore { result, more });
    }

    pub fn stop_fetch(&mut self, result: ResultId) {
        self.core.send(Command::StopFetch { result });
    }

    pub fn select_result(&mut self, tab: TabId, ix: usize, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(tab) {
            t.active_result = ix.min(t.results.len().saturating_sub(1));
        }
        cx.notify();
    }

    // ---- スキーマツリー ----

    pub(super) fn request_schemas_for(&mut self, tab: TabId) {
        if let Some(t) = self.tab(tab) {
            let id = t.connection.id.clone();
            self.schemas.entry(id.clone()).or_default();
            self.core.send(Command::LoadSchemas { connection_id: id });
        }
    }

    pub fn refresh_schema(&mut self, connection_id: ConnectionId) {
        self.core.send(Command::RefreshSchema { connection_id });
    }

    /// 開いている枝を読み直す(DDL の後、または更新の後)。
    pub(super) fn reload_expanded(&mut self, connection_id: &ConnectionId) {
        self.core.send(Command::LoadSchemas {
            connection_id: connection_id.clone(),
        });
        let Some(state) = self.schemas.get(connection_id) else {
            return;
        };
        let keys: Vec<String> = state.expanded.iter().cloned().collect();
        for key in keys {
            self.request_node(connection_id, &key);
        }
    }

    pub fn toggle_node(&mut self, connection_id: &ConnectionId, key: &str, cx: &mut Context<Self>) {
        let opened = self
            .schemas
            .entry(connection_id.clone())
            .or_default()
            .toggle(key);
        if opened {
            self.request_node(connection_id, key);
        }
        cx.notify();
    }

    fn request_node(&mut self, connection_id: &ConnectionId, key: &str) {
        let c = connection_id.clone();
        if let Some(schema) = key.strip_prefix("s:") {
            self.core.send(Command::LoadTables {
                connection_id: c,
                schema: schema.to_string(),
            });
        } else if let Some(rest) = key.strip_prefix("t:")
            && let Some((schema, table)) = rest.split_once(KEY_SEP)
        {
            self.core.send(Command::LoadTableDetails {
                connection_id: c,
                table: TableRef::new(schema, table),
            });
        }
    }
}
