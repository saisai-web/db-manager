//! 起動オプション(`--open`、`--run`)の実行。接続の一覧が届いたら接続を開き、開けたら SQL を実行する。

use gpui_kit::component::input::InputContent;
use gpui_kit::{Context, Window};
use sqltool_core::{Command, ConnectionSummary, TabId};

use super::AppModel;
use crate::launch::Launch;

impl AppModel {
    pub fn set_launch(&mut self, launch: Launch) {
        self.import_pending = launch.import.is_some();
        self.launch = launch;
    }

    /// 取り込みが終わった(または失敗した)。一覧を受信済みなら、接続を開く判断をする。
    pub(super) fn import_done(&mut self) {
        self.import_pending = false;
        if self.listed {
            let list = self.connections.clone();
            self.launch_open(&list);
        }
    }

    /// 一覧が届いたとき。`--open` の名前に合う接続を、一度だけ開く。
    pub(super) fn launch_open(&mut self, list: &[ConnectionSummary]) {
        let Some(name) = self.launch.open.clone() else {
            return;
        };
        match list.iter().find(|c| c.name == name) {
            Some(c) => {
                self.launch.open = None;
                self.core.send(Command::OpenTab {
                    connection_id: c.id.clone(),
                });
            }
            // 取り込み中は、まだ一覧に載っていないだけかもしれない
            None if self.import_pending => {}
            None => {
                self.launch.open = None;
                self.notice = Some(super::StatusLine::error(format!(
                    "接続「{name}」が見つかりません"
                )));
            }
        }
    }

    /// タブが開いたとき。`--run` の SQL をエディタに入れて実行する。
    pub(super) fn launch_run(&mut self, tab: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = self.launch.table.take() {
            let (schema, table) = name.split_once('.').unwrap_or(("public", name.as_str()));
            self.open_table(tab, sqltool_core::TableRef::new(schema, table));
        }
        if let Some(schema) = self.launch.er.take() {
            self.show_er(tab, schema, cx);
        }
        if let Some(sql) = self.launch.explain.take() {
            if let Some(t) = self.tab(tab) {
                t.editor
                    .update(cx, |e, cx| e.set_value(InputContent::from(sql), window, cx));
            }
            self.explain(tab, true, cx);
        }
        let Some(sql) = self.launch.run.take() else {
            return;
        };
        if let Some(t) = self.tab(tab) {
            t.editor
                .update(cx, |e, cx| e.set_value(InputContent::from(sql), window, cx));
        }
        self.run(tab, true, cx);
    }
}
