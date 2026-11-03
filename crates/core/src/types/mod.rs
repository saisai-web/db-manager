//! UI と core のあいだの契約。UI は [`Command`] を送り、[`Event`] を受け取る。

pub use data_io::{CsvOptions, ReadOptions as CsvReadOptions, TextEncoding};
pub use driver_api::{
    ColumnInfo, ErrorKind, IndexInfo, KeyInfo, KeyKind, ResultColumn, SchemaInfo, Secret,
    TableInfo, TableKind, TableRef, ValueClass,
};
pub use result_store::{SortOrder, StoreError};
pub use storage::{
    ConnectionOptions, EnvLabel, HistoryEntry, HistoryQuery, HistoryStatus, Snippet, TlsSetting,
};

pub type ConnectionId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResultId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfirmId(pub u64);

/// 保存を待っている編集(プレビュー済み)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanId(pub u64);

/// 画面に出せるエラー。接続文字列やパスワード、セルの値は含まない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserError {
    pub kind: ErrorKind,
    pub message: String,
    /// SQLSTATE など
    pub code: Option<String>,
    pub detail: Option<String>,
    pub hint: Option<String>,
}

impl UserError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Other,
            message: message.into(),
            code: None,
            detail: None,
            hint: None,
        }
    }
}

impl From<driver_api::DriverError> for UserError {
    fn from(e: driver_api::DriverError) -> Self {
        Self {
            kind: e.kind,
            message: e.message,
            code: e.code,
            detail: e.detail,
            hint: e.hint,
        }
    }
}

impl From<storage::StorageError> for UserError {
    fn from(e: storage::StorageError) -> Self {
        Self::new(e.to_string())
    }
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

mod command;
mod connection;
mod event;
mod execution;

pub use command::*;
pub use connection::*;
pub use event::*;
pub use execution::*;
