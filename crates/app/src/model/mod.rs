//! 画面の状態(モデル)。core のイベントを写し、利用者の操作を core のコマンドにする。
//! 描画は `ui` が、このモデルを読んで行う。

mod actions;
mod apply;
mod apply_exec;
mod apply_schema;
mod catalog;
mod designer;
mod edit;
mod er;
mod explain;
mod io;
mod launch;
mod library;
mod pump;
mod schema;
mod ssh;
mod status;
mod tab;

use std::collections::HashMap;

use crate::launch::Launch;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, Entity};
use sqltool_core::{ConnectionId, ConnectionSummary, Core, EventStream, TabId, UserError};

pub use edit::EditPreviewState;
pub use er::graph_of;
pub use explain::ExplainState;
pub use io::{ImportPreviewState, default_mapping};
pub use library::{Panel, preview};
pub use pump::start_pump;
pub use schema::{SchemaState, schema_key, table_key};
pub use ssh::{HostKeyPrompt, Retry, is_host_key_prompt};
pub use status::{Level, StatusLine};
pub use tab::TabState;

pub struct AppModel {
    pub core: Core,
    pub connections: Vec<ConnectionSummary>,
    pub tabs: Vec<TabState>,
    pub active: Option<TabId>,
    pub schemas: HashMap<ConnectionId, SchemaState>,
    /// 接続テストの結果(接続ダイアログが読む)
    pub test_result: Option<Result<String, UserError>>,
    /// 初めて接続する SSH サーバーの、ホスト鍵の確認待ち
    pub host_key: Option<HostKeyPrompt>,
    /// 直前に始めた接続の操作(ホスト鍵の確認のあと、やり直す)
    last_attempt: Option<Retry>,
    /// タブに属さない通知(接続の保存・削除など)
    pub notice: Option<StatusLine>,
    /// 開いているパネル(履歴、スニペット)と、その内容
    pub panel: Option<Panel>,
    pub history: Vec<sqltool_core::HistoryEntry>,
    pub snippets: Vec<sqltool_core::Snippet>,
    /// パネルの入力欄(履歴の絞り込み、スニペットの名前)
    /// ER 設計の画面(閉じても設計は残る)
    pub designer: Option<Entity<crate::ui::Designer>>,
    pub library_input: Option<gpui_kit::Entity<gpui_kit::component::input::InputState>>,
    launch: Launch,
    import_pending: bool,
    /// 接続の一覧を受信済み
    listed: bool,
}

impl AppModel {
    /// モデルを作り、core からのイベントを受け取り始める。
    pub fn start(
        core: Core,
        events: EventStream,
        window: AnyWindowHandle,
        cx: &mut App,
    ) -> Entity<Self> {
        let model = cx.new(|_: &mut Context<Self>| Self {
            core: core.clone(),
            connections: Vec::new(),
            tabs: Vec::new(),
            active: None,
            schemas: HashMap::new(),
            test_result: None,
            host_key: None,
            last_attempt: None,
            notice: None,
            panel: None,
            history: Vec::new(),
            snippets: Vec::new(),
            designer: None,
            library_input: None,
            launch: Launch::default(),
            import_pending: false,
            listed: false,
        });
        start_pump(&model, events, window, cx);
        core.send(sqltool_core::Command::ListConnections);
        model
    }

    pub fn tab(&self, id: TabId) -> Option<&TabState> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn active_tab(&self) -> Option<&TabState> {
        self.active.and_then(|id| self.tab(id))
    }
}
