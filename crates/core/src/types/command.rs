//! UI から core へのコマンド。

use super::*;
use crate::edit::Change;
use crate::plan::ExecuteTarget;

#[derive(Debug)]
pub enum Command {
    // 接続設定
    ListConnections,
    SaveConnection(ConnectionInput),
    DeleteConnection {
        id: ConnectionId,
    },
    /// 保存せずに、接続できるか確かめる
    TestConnection(ConnectionInput),
    /// 利用者が確認した SSH サーバーのホスト鍵(指紋)を、信頼済みとして記録する
    TrustSshHost {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// 記録したホスト鍵を忘れる(サーバーを入れ替えたときなど)
    ForgetSshHost {
        host: String,
        port: u16,
    },
    ExportConnections,
    ImportConnections {
        json: String,
    },

    // タブ(セッション)
    OpenTab {
        connection_id: ConnectionId,
    },
    /// 切断された接続を開き直す。トランザクションは失われる
    Reconnect {
        tab: TabId,
    },
    /// `force` でなければ、未コミットのトランザクションや実行中のタブは閉じない
    CloseTab {
        tab: TabId,
        force: bool,
    },
    SetReadOnly {
        tab: TabId,
        read_only: bool,
    },

    // 実行
    Execute {
        tab: TabId,
        sql: String,
        target: ExecuteTarget,
        options: ExecuteOptions,
    },
    Confirm {
        id: ConfirmId,
        accept: bool,
    },
    /// 実行中ならクエリを止め、取得上限で止まっていれば打ち切る
    Cancel {
        tab: TabId,
    },
    /// 取得上限で止まった結果の続きを取得する
    FetchMore {
        result: ResultId,
        more: FetchMore,
    },
    /// 取得上限で止まった結果をここで打ち切る(後続の文は実行を続ける)
    StopFetch {
        result: ResultId,
    },
    SortResult {
        result: ResultId,
        column: usize,
        order: SortOrder,
    },
    ClearSort {
        result: ResultId,
    },
    /// 結果のメモリと一時ファイルを解放する
    CloseResult {
        result: ResultId,
    },

    // テーブル編集
    /// スキーマツリーのテーブルを開く(全行を閲覧し、キーがあれば編集できる)。
    OpenTable {
        tab: TabId,
        table: TableRef,
    },
    /// 溜めた変更から SQL を作り、プレビューを返す。DB にはまだ何も送らない。
    PrepareEdit {
        tab: TabId,
        result: ResultId,
        changes: Vec<Change>,
    },
    /// プレビューを承認して、1 つのトランザクションとして実行する。
    CommitEdit {
        tab: TabId,
        plan: PlanId,
    },
    DiscardEdit {
        plan: PlanId,
    },

    // 実行計画
    /// カーソル位置の文(または選択範囲の最初の文)の実行計画を求める。`analyze` なら、実際に実行して
    /// 実測の時間を取る(書き込みを含む文には使えない)。
    Explain {
        tab: TabId,
        sql: String,
        target: ExecuteTarget,
        analyze: bool,
    },

    // 入出力
    /// 結果を、いま画面に見えている並び順のまま、ファイルに書き出す。
    ExportResult {
        result: ResultId,
        path: std::path::PathBuf,
        format: ExportFormat,
    },
    CancelExport {
        result: ResultId,
    },
    /// 取り込む CSV の先頭を読んで、見出し、文字コード、区切り文字を確かめる。
    PreviewCsv {
        path: std::path::PathBuf,
        options: CsvReadOptions,
    },
    /// CSV をテーブルに取り込む。全体が 1 つのトランザクションで、1 行でも失敗すれば、すべて取り消す。
    ImportCsv {
        tab: TabId,
        table: TableRef,
        path: std::path::PathBuf,
        options: CsvReadOptions,
        /// (CSV の列の位置, テーブルの列名)
        mapping: Vec<(usize, String)>,
    },

    // スキーマ
    LoadSchemas {
        connection_id: ConnectionId,
    },
    LoadTables {
        connection_id: ConnectionId,
        schema: String,
    },
    LoadTableDetails {
        connection_id: ConnectionId,
        table: TableRef,
    },
    /// キャッシュを捨てる。UI は続けて `LoadSchemas` を送る
    RefreshSchema {
        connection_id: ConnectionId,
    },

    // スニペット
    ListSnippets,
    /// `id` があれば更新、なければ新規。
    SaveSnippet {
        id: Option<String>,
        name: String,
        sql: String,
    },
    DeleteSnippet {
        id: String,
    },

    // 履歴
    ListHistory(HistoryQuery),
    ClearHistory {
        connection_id: Option<ConnectionId>,
    },

    /// すべてのセッションを閉じる。未コミットのトランザクションは破棄される
    Shutdown,
}

impl Command {
    /// `CommandFailed` に載せる、ログや画面向けの名前。引数の中身(SQL、パスワード)は含めない。
    pub fn name(&self) -> &'static str {
        match self {
            Self::ListConnections => "ListConnections",
            Self::SaveConnection(_) => "SaveConnection",
            Self::DeleteConnection { .. } => "DeleteConnection",
            Self::TestConnection(_) => "TestConnection",
            Self::TrustSshHost { .. } => "TrustSshHost",
            Self::ForgetSshHost { .. } => "ForgetSshHost",
            Self::ExportConnections => "ExportConnections",
            Self::ImportConnections { .. } => "ImportConnections",
            Self::OpenTab { .. } => "OpenTab",
            Self::Reconnect { .. } => "Reconnect",
            Self::CloseTab { .. } => "CloseTab",
            Self::SetReadOnly { .. } => "SetReadOnly",
            Self::Execute { .. } => "Execute",
            Self::Confirm { .. } => "Confirm",
            Self::Cancel { .. } => "Cancel",
            Self::FetchMore { .. } => "FetchMore",
            Self::StopFetch { .. } => "StopFetch",
            Self::SortResult { .. } => "SortResult",
            Self::ClearSort { .. } => "ClearSort",
            Self::CloseResult { .. } => "CloseResult",
            Self::OpenTable { .. } => "OpenTable",
            Self::PrepareEdit { .. } => "PrepareEdit",
            Self::CommitEdit { .. } => "CommitEdit",
            Self::DiscardEdit { .. } => "DiscardEdit",
            Self::Explain { .. } => "Explain",
            Self::ExportResult { .. } => "ExportResult",
            Self::CancelExport { .. } => "CancelExport",
            Self::PreviewCsv { .. } => "PreviewCsv",
            Self::ImportCsv { .. } => "ImportCsv",
            Self::LoadSchemas { .. } => "LoadSchemas",
            Self::LoadTables { .. } => "LoadTables",
            Self::LoadTableDetails { .. } => "LoadTableDetails",
            Self::RefreshSchema { .. } => "RefreshSchema",
            Self::ListSnippets => "ListSnippets",
            Self::SaveSnippet { .. } => "SaveSnippet",
            Self::DeleteSnippet { .. } => "DeleteSnippet",
            Self::ListHistory(_) => "ListHistory",
            Self::ClearHistory { .. } => "ClearHistory",
            Self::Shutdown => "Shutdown",
        }
    }
}
