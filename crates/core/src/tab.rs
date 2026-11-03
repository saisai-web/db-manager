use std::sync::{Arc, Mutex, MutexGuard};

use driver_api::{Canceller, Driver, Session};

use crate::plan::TxnState;
use crate::types::{ConfirmId, ConnectionSummary, ResultId, RunState, TabId};

/// エディタタブ 1 つぶん。Session は 1 つで、トランザクションはタブ単位。
pub(crate) struct Tab {
    pub id: TabId,
    pub connection: ConnectionSummary,
    pub driver: Arc<dyn Driver>,
    /// 実行中は、実行するタスクが保持する。切断されると `None`
    pub session: tokio::sync::Mutex<Option<Box<dyn Session>>>,
    canceller: Mutex<Arc<dyn Canceller>>,
    state: Mutex<TabRuntime>,
}

pub(crate) struct TabRuntime {
    pub run: RunState,
    pub txn: TxnState,
    pub read_only: bool,
    /// 実行の前に、利用者の確認を待っている
    pub pending_confirm: Option<ConfirmId>,
    /// 取得上限での一時停止を解除する先
    pub active_result: Option<ResultId>,
    /// 利用者が停止を求めた。実行が終わるまで有効
    pub cancel_requested: bool,
    /// 文の通し番号。キャンセルの再送が、次の文を止めてしまわないための世代
    pub statement_seq: u64,
    /// 文を DB に送り、終わりを待っている間だけ真
    pub in_statement: bool,
}

impl Tab {
    pub fn new(
        id: TabId,
        connection: ConnectionSummary,
        driver: Arc<dyn Driver>,
        session: Box<dyn Session>,
        read_only: bool,
    ) -> Self {
        let canceller: Arc<dyn Canceller> = Arc::from(session.canceller());
        Self {
            id,
            connection,
            driver,
            session: tokio::sync::Mutex::new(Some(session)),
            canceller: Mutex::new(canceller),
            state: Mutex::new(TabRuntime {
                run: RunState::Idle,
                txn: TxnState::Idle,
                read_only,
                pending_confirm: None,
                active_result: None,
                cancel_requested: false,
                statement_seq: 0,
                in_statement: false,
            }),
        }
    }

    pub fn rt(&self) -> MutexGuard<'_, TabRuntime> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn canceller(&self) -> Arc<dyn Canceller> {
        self.canceller
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn replace_canceller(&self, c: Arc<dyn Canceller>) {
        *self.canceller.lock().unwrap_or_else(|e| e.into_inner()) = c;
    }
}
