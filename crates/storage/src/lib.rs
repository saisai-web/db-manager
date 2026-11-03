//! アプリ内の設定・履歴(SQLite)と、パスワードなどの認証情報(OS の資格情報ストア)。
//!
//! パスワードは [`SecretStore`] にだけ保存し、SQLite には参照キー(`secret_ref`)しか持たない。
//! SQLite の接続は同期 API で `Send` だが `Sync` ではないため、呼び出し側(core)が
//! 専用スレッドで所有する。

mod db;
mod model;
mod paths;
mod secret;

pub use db::Storage;
pub use model::{
    ConnectionOptions, ConnectionRecord, EnvLabel, HistoryEntry, HistoryQuery, HistoryStatus,
    NewConnection, NewHistory, Snippet, TlsSetting,
};
pub use paths::default_data_dir;
pub use secret::{KeyringSecretStore, MemorySecretStore, SecretStore};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("アプリ内データベースの操作に失敗しました: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("ファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("資格情報ストアの操作に失敗しました: {0}")]
    Secret(String),
    #[error("データが不正です: {0}")]
    InvalidData(String),
    #[error(
        "このデータベースは新しい版のアプリで作られています(スキーマ版 {found}、対応は {supported} まで)"
    )]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("見つかりません: {0}")]
    NotFound(String),
}

pub type Result<T, E = StorageError> = std::result::Result<T, E>;
