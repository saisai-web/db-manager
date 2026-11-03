use std::collections::HashMap;
use std::sync::Mutex;

use crate::{Result, StorageError};

/// パスワードなどの保存先。実装は OS の資格情報ストアか、テスト用のメモリ。
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, secret: &str) -> Result<()>;
    /// 登録がなければ `Ok(None)`
    fn get(&self, key: &str) -> Result<Option<String>>;
    /// 登録がなくてもエラーにしない
    fn delete(&self, key: &str) -> Result<()>;
}

/// macOS の Keychain、Windows の資格情報マネージャー。
#[derive(Debug, Clone)]
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(secret_error)
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new("sqltool")
    }
}

fn secret_error(e: keyring::Error) -> StorageError {
    StorageError::Secret(e.to_string())
}

impl SecretStore for KeyringSecretStore {
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.entry(key)?.set_password(secret).map_err(secret_error)
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(secret_error(e)),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(secret_error(e)),
        }
    }
}

/// テストと、資格情報ストアのない環境用。プロセスの終了で消える。
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SecretStore for MemorySecretStore {
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.lock().insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.lock().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let s = MemorySecretStore::new();
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "1").unwrap();
        s.set("a", "2").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("2"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        assert!(s.is_empty());
    }

    /// 実際の OS の資格情報ストアに書き込むため、通常のテストでは実行しない。
    /// `cargo test -p storage -- --ignored keyring` で手動実行する(macOS では許可を求められることがある)。
    #[test]
    #[ignore = "OS の資格情報ストアを実際に操作する"]
    fn keyring_round_trip() {
        let store = KeyringSecretStore::new("sqltool-test");
        let key = format!("test-{}", std::process::id());
        assert_eq!(store.get(&key).unwrap(), None);
        store.set(&key, "pässwörd 日本語").unwrap();
        assert_eq!(store.get(&key).unwrap().as_deref(), Some("pässwörd 日本語"));
        store.delete(&key).unwrap();
        assert_eq!(store.get(&key).unwrap(), None);
        store.delete(&key).unwrap();
    }
}
