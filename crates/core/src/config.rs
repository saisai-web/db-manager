use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use driver_api::Driver;
use storage::SecretStore;

/// ドライバ名(`connections.driver` の値)から実装への対応。
/// 使うドライバは、起動する側が Cargo feature に応じて登録する。
#[derive(Clone, Default)]
pub struct DriverRegistry {
    drivers: HashMap<String, Arc<dyn Driver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: impl Into<String>, driver: Arc<dyn Driver>) -> Self {
        self.drivers.insert(name.into(), driver);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Driver>> {
        self.drivers.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<_> = self.drivers.keys().cloned().collect();
        v.sort();
        v
    }
}

pub struct CoreConfig {
    /// アプリ内 SQLite のファイル。`None` ならメモリ上(テスト用)
    pub database_path: Option<PathBuf>,
    pub secrets: Arc<dyn SecretStore>,
    pub drivers: DriverRegistry,
    /// 結果の退避先。`None` なら退避せず、メモリ上限で取得を止める
    /// (個人情報を扱う環境で、検索結果を平文でディスクに残さないための設定)
    pub spill_dir: Option<PathBuf>,
    /// 結果 1 つあたりのメモリ上限(バイト)。既定 512 MiB
    pub memory_limit: usize,
    /// 受信を一時停止する行数の既定値
    pub default_fetch_limit: u64,
    /// 接続ごとに残す実行履歴の件数
    pub history_keep: usize,
}

impl CoreConfig {
    pub const DEFAULT_FETCH_LIMIT: u64 = 100_000;

    pub fn new(secrets: Arc<dyn SecretStore>, drivers: DriverRegistry) -> Self {
        Self {
            database_path: None,
            secrets,
            drivers,
            spill_dir: None,
            memory_limit: result_store::StoreConfig::DEFAULT_MEMORY_LIMIT,
            default_fetch_limit: Self::DEFAULT_FETCH_LIMIT,
            history_keep: 10_000,
        }
    }

    /// デスクトップアプリの既定。データは OS ごとの保存先(`~/Library/Application Support/SQLTool`、
    /// `%APPDATA%\SQLTool`)に置き、パスワードは OS の資格情報ストアに保存する。
    /// 結果の退避先は保存先の `spill`(所有者だけが読めるディレクトリ)。
    ///
    /// 環境変数: `SQLTOOL_DATA_DIR` で保存先を変える。`SQLTOOL_NO_KEYCHAIN` を設定すると、
    /// 資格情報ストアのない環境向けに、パスワードをメモリにだけ置く(保存しない)。
    pub fn desktop(drivers: DriverRegistry) -> Result<Self, String> {
        let dir = std::env::var_os("SQLTOOL_DATA_DIR")
            .map(PathBuf::from)
            .or_else(storage::default_data_dir)
            .ok_or("データの保存先を決められません(HOME / APPDATA が未設定)")?;
        let secrets: Arc<dyn SecretStore> = if std::env::var_os("SQLTOOL_NO_KEYCHAIN").is_some() {
            Arc::new(storage::MemorySecretStore::new())
        } else {
            Arc::new(storage::KeyringSecretStore::default())
        };
        let mut cfg = Self::new(secrets, drivers);
        cfg.database_path = Some(dir.join("app.db"));
        cfg.spill_dir = Some(dir.join("spill"));
        Ok(cfg)
    }
}
