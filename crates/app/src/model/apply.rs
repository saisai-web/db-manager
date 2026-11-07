//! core のイベントを、モデルに適用する(接続設定とタブ)。実行と結果は `apply_exec`、スキーマは `apply_schema`。

use gpui_kit::{Context, Window};
use sqltool_core::{CloseBlock, Event, TabId};

use super::AppModel;
use super::status::StatusLine;
use super::tab::{PendingConfirm, TabState};

impl AppModel {
    pub(super) fn apply(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Connections(list) => {
                // 取り込みが先に終わっていた場合に備え、取り込み済みの分も合わせて判断する
                self.listed = true;
                let mut merged = list;
                for c in std::mem::take(&mut self.connections) {
                    if !merged.iter().any(|x| x.id == c.id) {
                        merged.push(c);
                    }
                }
                self.connections = merged;
                let all = self.connections.clone();
                self.launch_open(&all);
            }
            Event::ConnectionsImported(list) => {
                for c in list {
                    match self.connections.iter_mut().find(|x| x.id == c.id) {
                        Some(slot) => *slot = c,
                        None => self.connections.push(c),
                    }
                }
                self.connections.sort_by_key(|c| c.name.to_lowercase());
                self.import_done();
            }
            Event::ConnectionSaved(c) => {
                match self.connections.iter_mut().find(|x| x.id == c.id) {
                    Some(slot) => *slot = c,
                    None => self.connections.push(c),
                }
                self.connections.sort_by_key(|c| c.name.to_lowercase());
                self.notice = Some(StatusLine::ok("接続を保存しました"));
            }
            Event::ConnectionDeleted { id } => {
                self.connections.retain(|c| c.id != id);
                self.notice = Some(StatusLine::info("接続を削除しました"));
            }
            Event::ConnectionTested(result) => self.test_result = Some(result),
            Event::SshHostKeyUnconfirmed {
                host,
                port,
                fingerprint,
            } => self.host_key_unconfirmed(host, port, fingerprint),
            Event::SshHostKeyForgotten => {
                self.notice = Some(StatusLine::ok(
                    "登録したホスト鍵を忘れました。次の接続で、確認し直します",
                ));
            }
            Event::TabOpened {
                tab,
                connection,
                read_only,
                server_version,
            } => self.on_tab_opened(tab, connection, read_only, server_version, window, cx),
            Event::SqlGenerated {
                tab,
                table,
                kind,
                sql,
            } => self.on_sql_generated(tab, &table, kind, &sql, window, cx),
            Event::TabClosed { tab } => self.on_tab_closed(tab),
            Event::CloseBlocked { tab, reason } => {
                let text = match reason {
                    CloseBlock::Running => "実行中のため閉じられません。停止してから閉じてください",
                    CloseBlock::PendingTransaction(_) => {
                        "未コミットのトランザクションがあります。COMMIT か ROLLBACK してから閉じてください"
                    }
                };
                self.set_status(tab, StatusLine::warn(text));
            }
            Event::TabDisconnected { tab, error } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.connected = false;
                }
                self.set_status(
                    tab,
                    StatusLine::error(format!("{}。再接続してください", error)),
                );
            }
            Event::ReadOnlyChanged { tab, read_only } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.read_only = read_only;
                }
            }
            Event::TransactionChanged { tab, state } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.txn = state;
                }
            }
            Event::RunStateChanged { tab, state } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.run = state;
                }
            }
            Event::ConfirmationRequired { id, tab, items } => {
                if let Some(t) = self.tab_mut(tab) {
                    t.pending_confirm = Some(PendingConfirm { id, items });
                }
            }
            Event::CommandFailed {
                command,
                tab,
                error,
            } => {
                if command == "ImportConnections" {
                    self.import_done();
                }
                // ホスト鍵の確認は、エラーでなく確認の画面として出す
                if super::is_host_key_prompt(&error) {
                    return;
                }
                let line = StatusLine::error(format!("{command}: {error}"));
                match tab {
                    Some(t) => self.set_status(t, line),
                    None => self.notice = Some(line),
                }
            }
            e @ (Event::ExecutionStarted { .. }
            | Event::StatementStarted { .. }
            | Event::ResultOpened { .. }
            | Event::Progress { .. }
            | Event::FetchPaused { .. }
            | Event::StatementFinished { .. }
            | Event::ExecutionFinished { .. }
            | Event::SortChanged { .. }) => self.apply_exec(e, window, cx),
            e @ (Event::ExportProgress { .. }
            | Event::ExportFinished { .. }
            | Event::CsvPreview { .. }
            | Event::ImportProgress { .. }
            | Event::ImportFinished { .. }
            | Event::ImportFailed { .. }) => self.apply_io_event(e, cx),
            e @ (Event::EditPreview { .. }
            | Event::EditCommitted { .. }
            | Event::EditRejected { .. }) => self.apply_edit_event(e, cx),
            e @ (Event::Schemas { .. }
            | Event::Tables { .. }
            | Event::TableDetails { .. }
            | Event::SchemaInvalidated { .. }) => self.apply_schema(e),
            Event::ExplainReady { .. }
            | Event::History(_)
            | Event::Snippets(_)
            | Event::ConnectionsExported(_)
            | Event::ResultClosed { .. }
            | Event::ShutdownComplete => {}
        }
        cx.notify();
    }

    pub(super) fn set_status(&mut self, tab: TabId, line: StatusLine) {
        if let Some(t) = self.tab_mut(tab) {
            t.status = line;
        }
    }

    fn on_tab_opened(
        &mut self,
        id: TabId,
        connection: sqltool_core::ConnectionSummary,
        read_only: bool,
        server_version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let version = server_version
            .split(" on ")
            .next()
            .unwrap_or_default()
            .to_string();
        if let Some(t) = self.tab_mut(id) {
            // 再接続
            t.connected = true;
            t.read_only = read_only;
            t.server_version = version.into();
            t.status = StatusLine::ok("再接続しました");
            return;
        }
        let editor = self.new_query_editor(&connection, window, cx);
        self.tabs.push(TabState {
            id,
            connection,
            server_version: version.into(),
            read_only,
            txn: Default::default(),
            run: sqltool_core::RunState::Idle,
            queries: vec![super::tab::QueryDoc {
                title: "クエリ 1".into(),
                editor,
            }],
            current_query: 0,
            next_query_no: 2,
            results: Vec::new(),
            active_result: 0,
            status: StatusLine::ok("接続しました"),
            pending_confirm: None,
            edit_preview: None,
            import_preview: None,
            er: None,
            connected: true,
            table_limit: None,
        });
        self.active = Some(id);
        self.request_schemas_for(id);
        self.launch_run(id, window, cx);
    }

    fn on_tab_closed(&mut self, id: TabId) {
        self.tabs.retain(|t| t.id != id);
        if self.active == Some(id) {
            self.active = self.tabs.last().map(|t| t.id);
        }
    }
}
