use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use driver_api::{Driver, ErrorKind};
use storage::SecretStore;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::config::{CoreConfig, DriverRegistry};
use crate::connections::summarize_connection;
use crate::edit::EditAccess;
use crate::plan::TxnState;
use crate::result::ResultHandle;
use crate::rules::scan_rules;
use crate::schema::SchemaCache;
use crate::sink::FetchCommand;
use crate::storage_actor::StorageHandle;
use crate::tab::Tab;
use crate::types::*;

/// core の入口。UI は [`Core::send`] でコマンドを送り、[`EventStream`] でイベントを受け取る。
///
/// すべての処理は Tokio のランタイム上で動き、`send` は待たずに戻る。DB 通信もファイル I/O も
/// 呼び出したスレッドでは行わないため、UI スレッドから呼んでよい。
#[derive(Clone)]
pub struct Core {
    sh: Arc<Shared>,
}

/// core が出すイベントの受信側。
pub struct EventStream {
    rx: UnboundedReceiver<Event>,
}

impl EventStream {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    /// Tokio のランタイム外のスレッド向け。
    pub fn blocking_recv(&mut self) -> Option<Event> {
        self.rx.blocking_recv()
    }
}

impl Core {
    /// アプリ内データベースを開き、core を起動する。`runtime` 上でタスクを動かす。
    pub fn start(config: CoreConfig, runtime: Handle) -> Result<(Core, EventStream), UserError> {
        let storage = StorageHandle::spawn(config.database_path.as_deref())?;
        let (events, rx) = unbounded_channel();
        let sh = Arc::new(Shared {
            rt: runtime,
            drivers: config.drivers,
            secrets: config.secrets,
            spill_dir: config.spill_dir,
            memory_limit: config.memory_limit,
            default_fetch_limit: config.default_fetch_limit,
            history_keep: config.history_keep,
            storage,
            events,
            next_id: AtomicU64::new(1),
            tabs: Mutex::default(),
            results: Mutex::default(),
            fetch_controls: Mutex::default(),
            confirmations: Mutex::default(),
            schemas: Mutex::default(),
            exports: Mutex::default(),
            edit_plans: Mutex::default(),
            tunnels: Default::default(),
            trusted_now: Mutex::default(),
        });
        Ok((Core { sh }, EventStream { rx }))
    }

    /// コマンドを送る。結果はイベントで返る。待たずに戻る。
    pub fn send(&self, command: Command) {
        if let Command::TrustSshHost {
            host,
            port,
            fingerprint,
        } = &command
        {
            self.sh.remember_trusted_host(host, *port, fingerprint);
        }
        let sh = Arc::clone(&self.sh);
        self.sh.rt.spawn(async move { sh.handle(command).await });
    }

    /// 開いている結果。UI が表示範囲の行を読むために使う。
    pub fn result(&self, id: ResultId) -> Option<ResultHandle> {
        self.sh.results().get(&id).cloned()
    }

    /// SQL の補完候補。字句の規則は、接続先のドライバ(`driver`)の Dialect から決まる。
    /// スキーマ情報(`catalog`)は呼び出し側がキャッシュから渡すので、DB には問い合わせない。
    pub fn suggest_completion(
        &self,
        driver: &str,
        sql: &str,
        offset: usize,
        catalog: &crate::completion::Catalog,
    ) -> crate::completion::Suggestion {
        let rules = self
            .sh
            .drivers
            .get(driver)
            .map(|d| scan_rules(d.dialect().lexical_features()))
            .unwrap_or_default();
        crate::completion::suggest(sql, offset, &rules, catalog)
    }

    /// 未コミットのトランザクションがあるタブ。アプリを閉じる前の警告に使う。
    pub fn pending_transactions(&self) -> Vec<(TabId, TxnState)> {
        let mut v: Vec<_> = self
            .sh
            .tabs()
            .values()
            .filter_map(|t| {
                let txn = t.rt().txn;
                txn.is_pending().then_some((t.id, txn))
            })
            .collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }
}

pub(crate) struct Shared {
    pub(crate) rt: Handle,
    pub(crate) drivers: DriverRegistry,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) spill_dir: Option<std::path::PathBuf>,
    pub(crate) memory_limit: usize,
    pub(crate) default_fetch_limit: u64,
    pub(crate) history_keep: usize,
    pub(crate) storage: StorageHandle,
    pub(crate) events: UnboundedSender<Event>,
    pub(crate) next_id: AtomicU64,
    pub(crate) tabs: Mutex<HashMap<TabId, Arc<Tab>>>,
    pub(crate) results: Mutex<HashMap<ResultId, ResultHandle>>,
    pub(crate) fetch_controls: Mutex<HashMap<ResultId, UnboundedSender<FetchCommand>>>,
    pub(crate) confirmations: Mutex<HashMap<ConfirmId, oneshot::Sender<bool>>>,
    pub(crate) schemas: Mutex<HashMap<ConnectionId, Arc<SchemaCache>>>,
    pub(crate) exports: Mutex<HashMap<ResultId, Arc<std::sync::atomic::AtomicBool>>>,
    pub(crate) edit_plans: Mutex<HashMap<PlanId, crate::edit_ops::PendingEdit>>,
    /// 接続ごとの SSH トンネル(タブとメタデータ用の接続で共有する)
    pub(crate) tunnels: crate::ssh::TunnelMap,
    /// 信頼した直後のホスト鍵。保存(非同期)が終わる前に続けて送られた接続でも、見えるようにする
    pub(crate) trusted_now: Mutex<std::collections::HashMap<String, String>>,
}

pub(crate) type Res<T = ()> = Result<T, UserError>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn command_tab(c: &Command) -> Option<TabId> {
    match c {
        Command::Reconnect { tab }
        | Command::CloseTab { tab, .. }
        | Command::SetReadOnly { tab, .. }
        | Command::Execute { tab, .. }
        | Command::OpenTable { tab, .. }
        | Command::GenerateSql { tab, .. }
        | Command::PrepareEdit { tab, .. }
        | Command::CommitEdit { tab, .. }
        | Command::ImportCsv { tab, .. }
        | Command::Explain { tab, .. }
        | Command::Cancel { tab } => Some(*tab),
        _ => None,
    }
}

impl Shared {
    pub(crate) fn tabs(&self) -> MutexGuard<'_, HashMap<TabId, Arc<Tab>>> {
        lock(&self.tabs)
    }

    pub(crate) fn results(&self) -> MutexGuard<'_, HashMap<ResultId, ResultHandle>> {
        lock(&self.results)
    }

    pub(crate) fn emit(&self, event: Event) {
        // 受信側が閉じていれば、UI が終了処理中
        let _ = self.events.send(event);
    }

    pub(crate) fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn tab(&self, id: TabId) -> Res<Arc<Tab>> {
        self.tabs()
            .get(&id)
            .cloned()
            .ok_or_else(|| UserError::new("タブが見つかりません"))
    }

    pub(crate) fn driver(&self, name: &str) -> Res<Arc<dyn Driver>> {
        self.drivers.get(name).ok_or_else(|| UserError {
            kind: ErrorKind::Unsupported,
            ..UserError::new(format!("ドライバ「{name}」は利用できません"))
        })
    }

    pub(crate) async fn handle(self: Arc<Self>, command: Command) {
        let name = command.name();
        let tab = command_tab(&command);
        if let Err(error) = self.dispatch(command).await {
            self.emit(Event::CommandFailed {
                command: name,
                tab,
                error,
            });
        }
    }

    pub(crate) async fn dispatch(self: &Arc<Self>, command: Command) -> Res {
        match command {
            Command::ListConnections => {
                let list = self.storage.call(|s| s.connections()).await?;
                self.emit(Event::Connections(
                    list.iter().map(summarize_connection).collect(),
                ));
            }
            Command::SaveConnection(input) => {
                let saved = self.save_connection(input).await?;
                self.emit(Event::ConnectionSaved(saved));
            }
            Command::DeleteConnection { id } => self.delete_connection(id).await?,
            Command::TestConnection(input) => {
                let result = self.test_connection(input).await;
                self.emit(Event::ConnectionTested(result));
            }
            Command::TrustSshHost {
                host,
                port,
                fingerprint,
            } => self.trust_ssh_host(&host, port, fingerprint).await?,
            Command::ForgetSshHost { host, port } => {
                self.forget_ssh_host(&host, port).await?;
                self.emit(Event::SshHostKeyForgotten);
            }
            Command::ExportConnections => {
                let json = self.storage.call(|s| s.export_connections()).await?;
                self.emit(Event::ConnectionsExported(json));
            }
            Command::ImportConnections { json } => {
                let list = self
                    .storage
                    .call(move |s| s.import_connections(&json))
                    .await?;
                self.emit(Event::ConnectionsImported(
                    list.iter().map(summarize_connection).collect(),
                ));
            }

            Command::OpenTab { connection_id } => self.open_tab(connection_id).await?,
            Command::Reconnect { tab } => self.reconnect(tab).await?,
            Command::CloseTab { tab, force } => self.close_tab(tab, force).await?,
            Command::SetReadOnly { tab, read_only } => self.set_read_only(tab, read_only).await?,

            Command::Execute {
                tab,
                sql,
                target,
                options,
            } => {
                self.execute(tab, sql, target, options, EditAccess::NotATable)
                    .await?
            }
            Command::OpenTable { tab, table, limit } => self.open_table(tab, table, limit).await?,
            Command::GenerateSql { tab, table, kind } => {
                self.generate_sql(tab, table, kind).await?
            }
            Command::PrepareEdit {
                tab,
                result,
                changes,
            } => self.prepare_edit(tab, result, changes)?,
            Command::CommitEdit { tab, plan } => self.commit_edit(tab, plan).await?,
            Command::DiscardEdit { plan } => {
                lock(&self.edit_plans).remove(&plan);
            }
            Command::Explain {
                tab,
                sql,
                target,
                analyze,
            } => self.explain(tab, sql, target, analyze).await?,
            Command::ExportResult {
                result,
                path,
                format,
            } => self.export_result(result, path, format)?,
            Command::CancelExport { result } => {
                if let Some(flag) = lock(&self.exports).get(&result) {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            Command::PreviewCsv { path, options } => self.preview_csv(path, options).await?,
            Command::ImportCsv {
                tab,
                table,
                path,
                options,
                mapping,
            } => self.import_csv(tab, table, path, options, mapping).await?,
            Command::Confirm { id, accept } => {
                if let Some(tx) = lock(&self.confirmations).remove(&id) {
                    let _ = tx.send(accept);
                }
            }
            Command::Cancel { tab } => self.cancel(tab)?,
            Command::FetchMore { result, more } => self.fetch_control(
                result,
                match more {
                    FetchMore::NextBatch => FetchCommand::NextBatch,
                    FetchMore::Rest => FetchCommand::Rest,
                },
            )?,
            Command::StopFetch { result } => self.fetch_control(result, FetchCommand::Stop)?,
            Command::SortResult {
                result,
                column,
                order,
            } => self.sort_result(result, column, order).await?,
            Command::ClearSort { result } => {
                self.result_handle(result)?.clear_sorted();
                self.emit(Event::SortChanged { result });
            }
            Command::CloseResult { result } => {
                self.results().remove(&result);
                lock(&self.fetch_controls).remove(&result);
                self.emit(Event::ResultClosed { result });
            }

            Command::LoadSchemas { connection_id } => {
                let cache = self.schema_cache(&connection_id);
                let connect = self.connector(connection_id.clone());
                let schemas = cache.schemas(&connect).await?;
                self.emit(Event::Schemas {
                    connection_id,
                    schemas,
                });
            }
            Command::LoadTables {
                connection_id,
                schema,
            } => {
                let cache = self.schema_cache(&connection_id);
                let connect = self.connector(connection_id.clone());
                let tables = cache.tables(&connect, &schema).await?;
                self.emit(Event::Tables {
                    connection_id,
                    schema,
                    tables,
                });
            }
            Command::LoadTableDetails {
                connection_id,
                table,
            } => {
                let cache = self.schema_cache(&connection_id);
                let connect = self.connector(connection_id.clone());
                let details = cache.details(&connect, &table).await?;
                self.emit(Event::TableDetails {
                    connection_id,
                    details,
                });
            }
            Command::RefreshSchema { connection_id } => {
                self.schema_cache(&connection_id).invalidate().await;
                self.emit(Event::SchemaInvalidated { connection_id });
            }

            Command::ListSnippets => {
                let list = self.storage.call(|s| s.snippets()).await?;
                self.emit(Event::Snippets(list));
            }
            Command::SaveSnippet { id, name, sql } => {
                if name.trim().is_empty() {
                    return Err(UserError::new("スニペットの名前を入力してください"));
                }
                self.storage
                    .call(move |s| s.save_snippet(id.as_deref(), name.trim(), &sql))
                    .await?;
                let list = self.storage.call(|s| s.snippets()).await?;
                self.emit(Event::Snippets(list));
            }
            Command::DeleteSnippet { id } => {
                self.storage.call(move |s| s.delete_snippet(&id)).await?;
                let list = self.storage.call(|s| s.snippets()).await?;
                self.emit(Event::Snippets(list));
            }
            Command::ListHistory(query) => {
                let list = self.storage.call(move |s| s.history(&query)).await?;
                self.emit(Event::History(list));
            }
            Command::ClearHistory { connection_id } => {
                self.storage
                    .call(move |s| s.clear_history(connection_id.as_deref()))
                    .await?;
                self.emit(Event::History(Vec::new()));
            }

            Command::Shutdown => self.shutdown().await,
        }
        Ok(())
    }
}
