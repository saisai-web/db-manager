//! 接続設定の保存・削除・テストと、パスワードの扱い。

use std::sync::Arc;
use std::time::Duration;

use crate::core::{Res, Shared, lock};
use crate::ssh::{has_ssh_secret, ssh_secret_key};
use crate::types::*;
use driver_api::{ConnectionConfig, TlsMode};
use storage::{ConnectionRecord, NewConnection};

impl Shared {
    pub(crate) async fn secret(&self, key: Option<String>) -> Res<Option<Secret>> {
        let Some(key) = key else { return Ok(None) };
        let secrets = Arc::clone(&self.secrets);
        // OS の資格情報ストアの呼び出しは同期で、ユーザーの確認を待つことがある
        let got = tokio::task::spawn_blocking(move || secrets.get(&key))
            .await
            .map_err(|e| UserError::new(e.to_string()))??;
        Ok(got.map(Secret::new))
    }

    pub(crate) async fn store_secret(&self, key: String, value: Secret) -> Res {
        let secrets = Arc::clone(&self.secrets);
        tokio::task::spawn_blocking(move || secrets.set(&key, value.expose()))
            .await
            .map_err(|e| UserError::new(e.to_string()))??;
        Ok(())
    }

    pub(crate) async fn delete_secret(&self, key: String) -> Res {
        let secrets = Arc::clone(&self.secrets);
        tokio::task::spawn_blocking(move || secrets.delete(&key))
            .await
            .map_err(|e| UserError::new(e.to_string()))??;
        Ok(())
    }

    pub(crate) async fn save_connection(
        self: &Arc<Self>,
        input: ConnectionInput,
    ) -> Res<ConnectionSummary> {
        if input.name.trim().is_empty() {
            return Err(UserError::new("接続名を入力してください"));
        }
        self.driver(&input.driver)?;
        let ConnectionInput {
            id,
            name,
            driver,
            host,
            port,
            database,
            username,
            options,
            env_label,
            password,
            ssh_secret,
        } = input;

        let (mut record, had_ssh_secret) = match id {
            None => {
                let new = NewConnection {
                    name,
                    driver,
                    host,
                    port,
                    database,
                    username,
                    secret_ref: None,
                    options,
                    env_label,
                };
                let mut rec = self
                    .storage
                    .call(move |s| s.create_connection(&new))
                    .await?;
                if let PasswordChange::Set(pw) = password {
                    let key = format!("connection:{}", rec.id);
                    if let Err(e) = self.store_secret(key.clone(), pw).await {
                        // パスワードを保存できなければ、パスワードなしの接続を残さない
                        let id = rec.id.clone();
                        let _ = self.storage.call(move |s| s.delete_connection(&id)).await;
                        return Err(e);
                    }
                    rec.secret_ref = Some(key);
                    let r = rec.clone();
                    rec = self.storage.call(move |s| s.update_connection(&r)).await?;
                }
                (rec, false)
            }
            Some(id) => {
                let lookup = id.clone();
                let existing = self
                    .storage
                    .call(move |s| s.connection(&lookup))
                    .await?
                    .ok_or_else(|| UserError::new("接続が見つかりません"))?;
                let had_ssh_secret = has_ssh_secret(&existing.options);
                let mut rec = existing.clone();
                rec.name = name;
                rec.driver = driver;
                rec.host = host;
                rec.port = port;
                rec.database = database;
                rec.username = username;
                rec.options = options;
                rec.env_label = env_label;
                match password {
                    PasswordChange::Keep => {}
                    PasswordChange::Set(pw) => {
                        let key = existing
                            .secret_ref
                            .clone()
                            .unwrap_or_else(|| format!("connection:{}", existing.id));
                        self.store_secret(key.clone(), pw).await?;
                        rec.secret_ref = Some(key);
                    }
                    PasswordChange::Clear => {
                        if let Some(key) = existing.secret_ref.clone() {
                            self.delete_secret(key).await?;
                        }
                        rec.secret_ref = None;
                    }
                }
                let saved = self
                    .storage
                    .call(move |s| s.update_connection(&rec))
                    .await?;
                // 設定が変わったかもしれないので、次の接続では、トンネルを開き直す
                self.close_tunnel(&saved.id).await;
                (saved, had_ssh_secret)
            }
        };
        if self
            .apply_ssh_secret(&mut record, had_ssh_secret, ssh_secret)
            .await?
        {
            let r = record.clone();
            record = self.storage.call(move |s| s.update_connection(&r)).await?;
        }
        Ok(summarize_connection(&record))
    }

    pub(crate) async fn delete_connection(self: &Arc<Self>, id: ConnectionId) -> Res {
        if self.tabs().values().any(|t| t.connection.id == id) {
            return Err(UserError::new(
                "この接続のタブが開いています。先に閉じてください",
            ));
        }
        let lookup = id.clone();
        let existing = self.storage.call(move |s| s.connection(&lookup)).await?;
        let target = id.clone();
        self.storage
            .call(move |s| s.delete_connection(&target))
            .await?;
        self.close_tunnel(&id).await;
        if existing
            .as_ref()
            .is_some_and(|r| has_ssh_secret(&r.options))
        {
            let _ = self.delete_secret(ssh_secret_key(&id)).await;
        }
        let cache = lock(&self.schemas).remove(&id);
        if let Some(cache) = cache {
            cache.close().await;
        }
        self.emit(Event::ConnectionDeleted { id });
        // 履歴などを消した後で、パスワードも消す。失敗したら利用者に知らせる
        if let Some(key) = existing.and_then(|r| r.secret_ref) {
            self.delete_secret(key).await.map_err(|e| UserError {
                message: format!(
                    "接続は削除しましたが、保存したパスワードを削除できませんでした: {}",
                    e.message
                ),
                ..e
            })?;
        }
        Ok(())
    }

    pub(crate) async fn test_connection(&self, input: ConnectionInput) -> Res<String> {
        let driver = self.driver(&input.driver)?;
        let password = match input.password {
            PasswordChange::Set(pw) => Some(pw),
            PasswordChange::Clear => None,
            PasswordChange::Keep => match input.id.clone() {
                Some(id) => {
                    let rec = self.storage.call(move |s| s.connection(&id)).await?;
                    self.secret(rec.and_then(|r| r.secret_ref)).await?
                }
                None => None,
            },
        };
        let ssh_secret = match input.ssh_secret {
            PasswordChange::Set(pw) => Some(pw),
            PasswordChange::Clear => None,
            PasswordChange::Keep => match input.id.as_deref() {
                Some(id) if self.ssh_secret_saved(id).await => {
                    self.secret(Some(ssh_secret_key(id))).await?
                }
                _ => None,
            },
        };
        let cfg = to_config(
            input.host,
            input.port,
            input.database,
            input.username,
            &input.options,
            password,
        );
        // テストだけのトンネル。確かめたら閉じる
        let (cfg, _tunnel) = self
            .through_tunnel(None, cfg, &input.options, ssh_secret)
            .await?;
        let session = driver.connect(&cfg).await?;
        let version = session.server_info().version.clone();
        let _ = session.close().await;
        Ok(version)
    }

    pub(crate) async fn connection_config(&self, rec: &ConnectionRecord) -> Res<ConnectionConfig> {
        let password = self.secret(rec.secret_ref.clone()).await?;
        let cfg = to_config(
            rec.host.clone(),
            rec.port,
            rec.database.clone(),
            rec.username.clone(),
            &rec.options,
            password,
        );
        let ssh_secret = if has_ssh_secret(&rec.options) {
            self.secret(Some(ssh_secret_key(&rec.id))).await?
        } else {
            None
        };
        // トンネルは接続ごとに 1 本、`Shared` が持つ。ここでは、つなぎ先の差し替えだけを受け取る
        let (cfg, _) = self
            .through_tunnel(Some(&rec.id), cfg, &rec.options, ssh_secret)
            .await?;
        Ok(cfg)
    }

    pub(crate) async fn record(&self, id: &str) -> Res<ConnectionRecord> {
        let id = id.to_string();
        self.storage
            .call(move |s| s.connection(&id))
            .await?
            .ok_or_else(|| UserError::new("接続が見つかりません"))
    }
}

pub(crate) fn summarize_connection(rec: &ConnectionRecord) -> ConnectionSummary {
    ConnectionSummary {
        id: rec.id.clone(),
        name: rec.name.clone(),
        driver: rec.driver.clone(),
        host: rec.host.clone(),
        port: rec.port,
        database: rec.database.clone(),
        username: rec.username.clone(),
        options: rec.options.clone(),
        env_label: rec.env_label,
        has_password: rec.secret_ref.is_some(),
    }
}

fn to_config(
    host: Option<String>,
    port: Option<u16>,
    database: Option<String>,
    username: Option<String>,
    options: &ConnectionOptions,
    password: Option<Secret>,
) -> ConnectionConfig {
    let mut cfg = ConnectionConfig::new(host.unwrap_or_else(|| "localhost".into()));
    cfg.port = port;
    cfg.database = database;
    cfg.username = username;
    cfg.password = password;
    // CA 証明書は、新しい版の設定項目として `extra` に保存されている
    cfg.ca_file = options
        .extra
        .get("tls_ca_file")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    cfg.tls = match options.tls {
        TlsSetting::Disable => TlsMode::Disable,
        TlsSetting::Verify => TlsMode::Verify,
        TlsSetting::SkipVerify => TlsMode::SkipVerify,
    };
    cfg.connect_timeout = Duration::from_secs(u64::from(options.connect_timeout_secs.max(1)));
    cfg.application_name = "SQLTool".into();
    cfg
}
