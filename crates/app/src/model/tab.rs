//! エディタタブ 1 つぶんの画面状態。DB の状態は持たず、core のイベントを写したものだけを持つ。

use gpui_kit::component::input::EditorState;
use gpui_kit::{Entity, SharedString};
use sqltool_core::{ConfirmId, ConfirmItem, ConnectionSummary, RunState, TabId, TxnState};

use super::status::StatusLine;
use crate::ui::ResultView;

/// 実行前の確認を待っている(WHERE なしの UPDATE / DELETE、DROP / TRUNCATE)。
#[derive(Clone)]
pub struct PendingConfirm {
    pub id: ConfirmId,
    pub items: Vec<ConfirmItem>,
}

pub struct TabState {
    pub id: TabId,
    pub connection: ConnectionSummary,
    pub server_version: SharedString,
    pub read_only: bool,
    pub txn: TxnState,
    pub run: RunState,
    pub editor: Entity<EditorState>,
    pub results: Vec<ResultView>,
    pub active_result: usize,
    pub status: StatusLine,
    pub pending_confirm: Option<PendingConfirm>,
    /// 保存前の確認(プレビュー)
    pub edit_preview: Option<super::EditPreviewState>,
    /// 実行計画の表示
    pub explain: Option<super::ExplainState>,
    /// ER 図で表示しているスキーマ
    pub er: Option<String>,
    /// 取り込み前の確認(プレビュー)
    pub import_preview: Option<super::ImportPreviewState>,
    pub connected: bool,
}

impl TabState {
    pub fn title(&self) -> SharedString {
        self.connection.name.clone().into()
    }

    pub fn is_production(&self) -> bool {
        self.connection.env_label == sqltool_core::EnvLabel::Production
    }

    pub fn is_idle(&self) -> bool {
        self.run == RunState::Idle
    }

    pub fn active_result(&self) -> Option<&ResultView> {
        self.results.get(self.active_result)
    }

    pub fn result_mut(&mut self, id: sqltool_core::ResultId) -> Option<&mut ResultView> {
        self.results.iter_mut().find(|r| r.handle.id() == id)
    }
}
