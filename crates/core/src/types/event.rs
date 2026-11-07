//! core から UI へのイベント。

use std::time::Duration;

use super::*;
use crate::plan::{ConfirmItem, TxnState};
use crate::result::ResultHandle;

#[derive(Debug, Clone)]
pub struct TableDetails {
    pub table: TableRef,
    pub columns: Vec<ColumnInfo>,
    pub keys: Vec<KeyInfo>,
    pub indexes: Vec<IndexInfo>,
}

/// 保存前に見せる SQL(値を埋め込んだもの)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPreviewItem {
    pub kind: crate::edit::ChangeKind,
    pub sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRejection {
    /// 影響行数が 1 でなかった。他の人が先に更新・削除した可能性がある。
    RowCount {
        statement_index: usize,
        actual: u64,
        sql: String,
    },
    Database {
        statement_index: usize,
        error: UserError,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
    // 接続設定
    Connections(Vec<ConnectionSummary>),
    ConnectionSaved(ConnectionSummary),
    ConnectionDeleted {
        id: ConnectionId,
    },
    /// 成功なら、サーバーの版
    ConnectionTested(Result<String, UserError>),
    /// 初めて接続する SSH サーバー。指紋を利用者に見せ、信頼するなら `TrustSshHost` を送って、やり直す
    SshHostKeyUnconfirmed {
        host: String,
        port: u16,
        fingerprint: String,
    },
    SshHostKeyForgotten,
    ConnectionsExported(String),
    ConnectionsImported(Vec<ConnectionSummary>),

    // タブ
    TabOpened {
        tab: TabId,
        connection: ConnectionSummary,
        read_only: bool,
        server_version: String,
    },
    TabClosed {
        tab: TabId,
    },
    CloseBlocked {
        tab: TabId,
        reason: CloseBlock,
    },
    /// 接続が切れた。`Reconnect` で開き直せる
    TabDisconnected {
        tab: TabId,
        error: UserError,
    },
    ReadOnlyChanged {
        tab: TabId,
        read_only: bool,
    },
    TransactionChanged {
        tab: TabId,
        state: TxnState,
    },
    RunStateChanged {
        tab: TabId,
        state: RunState,
    },

    // 実行
    ConfirmationRequired {
        id: ConfirmId,
        tab: TabId,
        items: Vec<ConfirmItem>,
    },
    ExecutionStarted {
        tab: TabId,
        statements: usize,
    },
    StatementStarted {
        tab: TabId,
        index: usize,
        /// エディタ全体でのバイト範囲
        range: std::ops::Range<usize>,
    },
    /// 結果グリッドを開く。列が決まった時点で通知される
    ResultOpened {
        tab: TabId,
        result: ResultHandle,
    },
    /// 受信中の行数。画面の更新に合わせて間引いてある
    Progress {
        result: ResultId,
        rows: u64,
        elapsed: Duration,
    },
    /// 取得上限で受信を止めた。`FetchMore` か `StopFetch` を待っている
    FetchPaused {
        result: ResultId,
        rows: u64,
    },
    StatementFinished {
        tab: TabId,
        index: usize,
        result: Option<ResultId>,
        outcome: StatementOutcome,
    },
    ExecutionFinished {
        tab: TabId,
        status: ExecutionStatus,
    },
    SortChanged {
        result: ResultId,
    },
    ResultClosed {
        result: ResultId,
    },

    // テーブル編集
    EditPreview {
        tab: TabId,
        plan: PlanId,
        statements: Vec<EditPreviewItem>,
    },
    EditCommitted {
        tab: TabId,
        plan: PlanId,
        statements: usize,
    },
    /// 保存できなかった。トランザクションは取り消してあり、DB は変わっていない。
    EditRejected {
        tab: TabId,
        plan: PlanId,
        reason: EditRejection,
    },

    /// `Command::GenerateSql` の結果
    SqlGenerated {
        tab: TabId,
        table: TableRef,
        kind: SqlKind,
        sql: String,
    },

    // 実行計画
    ExplainReady {
        tab: TabId,
        /// 実行計画を求めた文
        sql: String,
        analyze: bool,
        plan: crate::explain::Plan,
    },

    // 入出力
    ExportProgress {
        result: ResultId,
        rows: u64,
    },
    ExportFinished {
        result: ResultId,
        path: std::path::PathBuf,
        rows: u64,
        /// 結果の一部しか受信していない(打ち切りや、取得上限での停止)ため、全件ではない
        partial: bool,
    },
    CsvPreview {
        path: std::path::PathBuf,
        encoding: TextEncoding,
        delimiter: u8,
        headers: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
    },
    ImportProgress {
        tab: TabId,
        rows: u64,
    },
    ImportFinished {
        tab: TabId,
        rows: u64,
    },
    /// 取り込めなかった。トランザクションは取り消してあり、DB は変わっていない。
    ImportFailed {
        tab: TabId,
        error: UserError,
    },

    // スキーマ
    Schemas {
        connection_id: ConnectionId,
        schemas: Vec<SchemaInfo>,
    },
    Tables {
        connection_id: ConnectionId,
        schema: String,
        tables: Vec<TableInfo>,
    },
    TableDetails {
        connection_id: ConnectionId,
        details: TableDetails,
    },
    /// DDL の実行などで、キャッシュが古くなった
    SchemaInvalidated {
        connection_id: ConnectionId,
    },

    History(Vec<HistoryEntry>),
    Snippets(Vec<Snippet>),

    /// コマンドを実行できなかった
    CommandFailed {
        command: &'static str,
        tab: Option<TabId>,
        error: UserError,
    },
    ShutdownComplete,
}
