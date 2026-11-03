//! タブ(セッション)の開閉、再接続、読み取り専用モード。

use std::sync::Arc;

use async_trait::async_trait;
use driver_api::{Driver, Flow, ResultColumn, RowSink, Session, Statement};
use storage::ConnectionRecord;

use crate::connections::summarize_connection;
use crate::core::{Res, Shared, lock};
use crate::plan::TxnState;
use crate::tab::Tab;
use crate::types::*;

impl Shared {
    pub(crate) async fn open_session(
        &self,
        rec: &ConnectionRecord,
        read_only: bool,
    ) -> Res<(Arc<dyn Driver>, Box<dyn Session>)> {
        let driver = self.driver(&rec.driver)?;
        let cfg = self.connection_config(rec).await?;
        let mut session = driver.connect(&cfg).await?;
        if read_only {
            apply_read_only(&*driver, session.as_mut(), true).await?;
        }
        Ok((driver, session))
    }

    pub(crate) async fn open_tab(self: &Arc<Self>, connection_id: ConnectionId) -> Res {
        let rec = self.record(&connection_id).await?;
        // 本番ラベルの接続は、接続直後は読み取り専用にする
        let read_only = rec.env_label == EnvLabel::Production;
        let (driver, session) = self.open_session(&rec, read_only).await?;
        let version = session.server_info().version.clone();
        let id = TabId(self.id());
        let connection = summarize_connection(&rec);
        let tab = Arc::new(Tab::new(id, connection.clone(), driver, session, read_only));
        self.tabs().insert(id, tab);
        self.emit(Event::TabOpened {
            tab: id,
            connection,
            read_only,
            server_version: version,
        });
        Ok(())
    }

    pub(crate) async fn reconnect(self: &Arc<Self>, tab_id: TabId) -> Res {
        let tab = self.tab(tab_id)?;
        {
            let rt = tab.rt();
            if rt.run != RunState::Idle {
                return Err(UserError::new("実行中のタブは開き直せません"));
            }
        }
        let rec = self.record(&tab.connection.id).await?;
        let read_only = tab.rt().read_only;
        let (_, session) = self.open_session(&rec, read_only).await?;
        let version = session.server_info().version.clone();
        tab.replace_canceller(Arc::from(session.canceller()));
        *tab.session.lock().await = Some(session);
        let had_txn = std::mem::replace(&mut tab.rt().txn, TxnState::Idle).is_pending();
        if had_txn {
            self.emit(Event::TransactionChanged {
                tab: tab_id,
                state: TxnState::Idle,
            });
        }
        self.emit(Event::TabOpened {
            tab: tab_id,
            connection: tab.connection.clone(),
            read_only,
            server_version: version,
        });
        Ok(())
    }

    pub(crate) async fn close_tab(self: &Arc<Self>, tab_id: TabId, force: bool) -> Res {
        let tab = self.tab(tab_id)?;
        let (run, txn) = {
            let rt = tab.rt();
            (rt.run, rt.txn)
        };
        if !force {
            if run != RunState::Idle {
                self.emit(Event::CloseBlocked {
                    tab: tab_id,
                    reason: CloseBlock::Running,
                });
                return Ok(());
            }
            if txn.is_pending() {
                self.emit(Event::CloseBlocked {
                    tab: tab_id,
                    reason: CloseBlock::PendingTransaction(txn),
                });
                return Ok(());
            }
        } else if run != RunState::Idle {
            // 実行中のタブを閉じるときは、サーバーに停止を依頼してから切り離す
            let _ = self.cancel(tab_id);
        }
        self.tabs().remove(&tab_id);
        let owned: Vec<ResultId> = self
            .results()
            .iter()
            .filter(|(_, h)| h.tab() == tab_id)
            .map(|(id, _)| *id)
            .collect();
        for id in owned {
            self.results().remove(&id);
            lock(&self.fetch_controls).remove(&id);
        }
        if run == RunState::Idle
            && let Some(session) = tab.session.lock().await.take()
        {
            let _ = session.close().await;
        }
        // この接続を使うタブがなくなったら、SSH トンネルも閉じる
        let still_used = self
            .tabs()
            .values()
            .any(|t| t.connection.id == tab.connection.id);
        if !still_used {
            self.close_tunnel(&tab.connection.id).await;
        }
        self.emit(Event::TabClosed { tab: tab_id });
        Ok(())
    }

    pub(crate) async fn set_read_only(self: &Arc<Self>, tab_id: TabId, read_only: bool) -> Res {
        let tab = self.tab(tab_id)?;
        if tab.rt().run != RunState::Idle {
            return Err(UserError::new(
                "実行中は読み取り専用モードを切り替えられません",
            ));
        }
        let mut guard = tab.session.lock().await;
        let session = guard
            .as_mut()
            .ok_or_else(|| UserError::new("接続が切れています。再接続してください"))?;
        apply_read_only(&*tab.driver, session.as_mut(), read_only).await?;
        drop(guard);
        tab.rt().read_only = read_only;
        self.emit(Event::ReadOnlyChanged {
            tab: tab_id,
            read_only,
        });
        Ok(())
    }

    pub(crate) async fn shutdown(self: &Arc<Self>) {
        let tabs: Vec<Arc<Tab>> = self.tabs().drain().map(|(_, t)| t).collect();
        for tab in tabs {
            if tab.rt().run != RunState::Idle {
                let _ = tab.canceller().cancel().await;
            }
            if let Some(session) = tab.session.lock().await.take() {
                let _ = session.close().await;
            }
        }
        self.results().clear();
        let caches: Vec<_> = lock(&self.schemas).drain().map(|(_, c)| c).collect();
        for c in caches {
            c.close().await;
        }
        self.emit(Event::ShutdownComplete);
    }
}

/// 何も受け取らないシンク。設定用の文の実行に使う。
pub(crate) struct NullSink;

#[async_trait]
impl RowSink for NullSink {
    fn columns(&mut self, _: &[ResultColumn]) -> Flow {
        Flow::Continue
    }
    fn row(&mut self, _: &[Option<&[u8]>]) -> Flow {
        Flow::Continue
    }
    async fn resume(&mut self) -> Flow {
        Flow::Continue
    }
}

/// DB 側の読み取り専用の設定を切り替える。DB が対応していなければ何もしない
/// (sqlscan による判定は、DB 側の設定とは別に常に働く)。
async fn apply_read_only(driver: &dyn Driver, session: &mut dyn Session, read_only: bool) -> Res {
    if !driver.capabilities().read_only_session {
        return Ok(());
    }
    if let Some(sql) = driver.dialect().set_read_only_sql(read_only) {
        session.execute(&Statement::new(sql), &mut NullSink).await?;
    }
    Ok(())
}
