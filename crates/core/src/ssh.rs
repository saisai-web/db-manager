//! SSH トンネル経由の接続。設定の読み取り、ホスト鍵の信頼の記録、トンネルの再利用。
//!
//! パスワード(または鍵のパスフレーズ)は OS の資格情報ストアだけに置く。接続設定(`options`)に
//! 入るのは、ホスト名・ユーザー名・鍵ファイルの場所と、「秘密を保存している」という印だけ。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use driver_api::{ConnectionConfig, Secret};
use ssh_tunnel::{SshAuth, Tunnel, TunnelConfig, TunnelError};
use storage::{ConnectionOptions, ConnectionRecord};

use crate::core::{Res, Shared, lock};
use crate::types::*;

/// ホスト鍵が未登録(利用者の確認が要る)
pub const HOST_KEY_UNKNOWN: &str = "ssh-host-key-unknown";
/// ホスト鍵が登録済みのものと違う(接続しない)
pub const HOST_KEY_CHANGED: &str = "ssh-host-key-changed";

pub(crate) type TunnelMap = tokio::sync::Mutex<HashMap<ConnectionId, (SshSettings, Arc<Tunnel>)>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SshLogin {
    Password,
    KeyFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub login: SshLogin,
}

fn text(o: &ConnectionOptions, key: &str) -> String {
    o.extra
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

impl SshSettings {
    /// `options` の SSH の項目。使わない設定なら `None`。
    pub(crate) fn from_options(o: &ConnectionOptions) -> Res<Option<Self>> {
        if o.extra.get("ssh").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(None);
        }
        let (host, user) = (text(o, "ssh_host"), text(o, "ssh_user"));
        if host.is_empty() || user.is_empty() {
            return Err(UserError::new(
                "SSH トンネルを使うには、SSH サーバーとユーザー名を入力してください",
            ));
        }
        let port = match o.extra.get("ssh_port") {
            None => 22,
            Some(v) => v
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p != 0)
                .ok_or_else(|| UserError::new("SSH のポートが正しくありません"))?,
        };
        let login = match text(o, "ssh_auth").as_str() {
            "key" => {
                let path = text(o, "ssh_key_file");
                if path.is_empty() {
                    return Err(UserError::new("SSH の秘密鍵ファイルを指定してください"));
                }
                SshLogin::KeyFile(PathBuf::from(path))
            }
            _ => SshLogin::Password,
        };
        Ok(Some(Self {
            host,
            port,
            user,
            login,
        }))
    }
}

pub(crate) fn ssh_secret_key(connection_id: &str) -> String {
    format!("ssh:{connection_id}")
}

const SECRET_FLAG: &str = "ssh_secret";

/// 資格情報ストアに、この接続の SSH 用の秘密を保存しているか(接続設定の印)。
pub(crate) fn has_ssh_secret(o: &ConnectionOptions) -> bool {
    o.extra.get(SECRET_FLAG).and_then(|v| v.as_bool()) == Some(true)
}

/// 印を付け外しする。変えたなら `true`。
fn set_ssh_secret_flag(o: &mut ConnectionOptions, saved: bool) -> bool {
    if has_ssh_secret(o) == saved {
        return false;
    }
    if saved {
        o.extra.insert(SECRET_FLAG.into(), true.into());
    } else {
        o.extra.remove(SECRET_FLAG);
    }
    true
}

fn host_key_setting(host: &str, port: u16) -> String {
    format!("ssh_host_key:{}:{port}", host.to_lowercase())
}

impl Shared {
    pub(crate) async fn trusted_host_key(&self, host: &str, port: u16) -> Res<Option<String>> {
        let key = host_key_setting(host, port);
        if let Some(fingerprint) = lock(&self.trusted_now).get(&key) {
            return Ok(Some(fingerprint.clone()));
        }
        Ok(self.storage.call(move |s| s.setting(&key)).await?)
    }

    /// 信頼を、保存より先に覚える。信頼の直後にすぐ接続し直されても、鍵が未確認に見えないように
    /// (保存は別のタスクで行うので、そのままだと追い越されることがある)。
    pub(crate) fn remember_trusted_host(&self, host: &str, port: u16, fingerprint: &str) {
        lock(&self.trusted_now).insert(host_key_setting(host, port), fingerprint.to_string());
    }

    /// 利用者が確認した指紋を、そのサーバーの信頼済みホスト鍵として記録する。
    pub(crate) async fn trust_ssh_host(&self, host: &str, port: u16, fingerprint: String) -> Res {
        let key = host_key_setting(host, port);
        self.storage
            .call(move |s| s.set_setting(&key, &fingerprint))
            .await?;
        Ok(())
    }

    pub(crate) async fn forget_ssh_host(&self, host: &str, port: u16) -> Res {
        let key = host_key_setting(host, port);
        lock(&self.trusted_now).remove(&key);
        self.storage.call(move |s| s.delete_setting(&key)).await?;
        Ok(())
    }

    /// 接続設定に SSH トンネルが指定されていれば、トンネルを開き(開いていれば使い回し)、
    /// DB への接続先を `127.0.0.1:<トンネルのポート>` に差し替える。`host` は、TLS の検証名として残す。
    /// `connection_id` が `Some` なら、トンネルを接続ごとに 1 本だけ持つ(タブとメタデータ用の接続で共有する)。
    pub(crate) async fn through_tunnel(
        &self,
        connection_id: Option<&str>,
        mut cfg: ConnectionConfig,
        options: &ConnectionOptions,
        secret: Option<Secret>,
    ) -> Res<(ConnectionConfig, Option<Arc<Tunnel>>)> {
        let Some(ssh) = SshSettings::from_options(options)? else {
            return Ok((cfg, None));
        };
        let target_port = cfg.port.ok_or_else(|| {
            UserError::new("SSH トンネルを使うときは、データベースのポートを指定してください")
        })?;
        let mut map = self.tunnels.lock().await;
        let reusable = connection_id
            .and_then(|id| map.get(id))
            .filter(|(used, t)| *used == ssh && !t.is_closed())
            .map(|(_, t)| Arc::clone(t));
        let tunnel = match reusable {
            Some(t) => t,
            None => {
                let t = Arc::new(
                    self.open_tunnel(&ssh, &cfg.host, target_port, secret, &cfg)
                        .await?,
                );
                if let Some(id) = connection_id {
                    map.insert(id.to_string(), (ssh, Arc::clone(&t)));
                }
                t
            }
        };
        cfg.port = Some(tunnel.local_port());
        cfg.hostaddr = Some(std::net::Ipv4Addr::LOCALHOST.into());
        Ok((cfg, Some(tunnel)))
    }

    async fn open_tunnel(
        &self,
        ssh: &SshSettings,
        target_host: &str,
        target_port: u16,
        secret: Option<Secret>,
        cfg: &ConnectionConfig,
    ) -> Res<Tunnel> {
        let trusted = self.trusted_host_key(&ssh.host, ssh.port).await?;
        let auth = match &ssh.login {
            SshLogin::Password => SshAuth::Password(secret.unwrap_or_else(|| Secret::new(""))),
            SshLogin::KeyFile(path) => SshAuth::KeyFile {
                path: path.clone(),
                passphrase: secret.filter(|s| !s.expose().is_empty()),
            },
        };
        let tunnel_cfg = TunnelConfig {
            ssh_host: ssh.host.clone(),
            ssh_port: ssh.port,
            user: ssh.user.clone(),
            auth,
            trusted_fingerprint: trusted,
            target_host: target_host.to_string(),
            target_port,
            connect_timeout: cfg.connect_timeout,
        };
        Tunnel::open(&tunnel_cfg)
            .await
            .map_err(|e| self.tunnel_error(ssh, e))
    }

    fn tunnel_error(&self, ssh: &SshSettings, e: TunnelError) -> UserError {
        let kind = match e {
            TunnelError::AuthFailed => ErrorKind::Auth,
            _ => ErrorKind::Connect,
        };
        let mut err = UserError {
            kind,
            ..UserError::new(e.to_string())
        };
        match e {
            TunnelError::HostKeyUnknown { fingerprint } => {
                err.code = Some(HOST_KEY_UNKNOWN.into());
                err.message = format!(
                    "SSH サーバー {}:{} のホスト鍵を確認してください",
                    ssh.host, ssh.port
                );
                err.detail = Some(fingerprint.clone());
                self.emit(Event::SshHostKeyUnconfirmed {
                    host: ssh.host.clone(),
                    port: ssh.port,
                    fingerprint,
                });
            }
            TunnelError::HostKeyChanged { .. } => {
                err.code = Some(HOST_KEY_CHANGED.into());
                err.hint = Some(
                    "サーバーが入れ替わった、または、なりすまされている可能性があります。\
                     サーバーの管理者に指紋を確認してください。意図した変更なら、接続設定の\
                     「ホスト鍵を忘れる」を実行してから、接続し直してください"
                        .into(),
                );
            }
            _ => {}
        }
        err
    }

    pub(crate) async fn ssh_secret_saved(&self, connection_id: &str) -> bool {
        let id = connection_id.to_string();
        match self.storage.call(move |s| s.connection(&id)).await {
            Ok(Some(rec)) => has_ssh_secret(&rec.options),
            _ => false,
        }
    }

    /// 保存した接続の SSH の秘密を、資格情報ストアに反映し、`options` の印を合わせる。
    /// SSH を使わない設定になったら、保存済みの秘密も消す。レコードを書き換えたなら `true`。
    pub(crate) async fn apply_ssh_secret(
        &self,
        rec: &mut ConnectionRecord,
        had: bool,
        change: PasswordChange,
    ) -> Res<bool> {
        let enabled = rec.options.extra.get("ssh").and_then(|v| v.as_bool()) == Some(true);
        let key = ssh_secret_key(&rec.id);
        let mut has = had;
        match change {
            PasswordChange::Set(pw) if enabled => {
                self.store_secret(key.clone(), pw).await?;
                has = true;
            }
            PasswordChange::Clear if has => {
                self.delete_secret(key.clone()).await?;
                has = false;
            }
            _ => {}
        }
        if !enabled && has {
            self.delete_secret(key).await?;
            has = false;
        }
        Ok(set_ssh_secret_flag(&mut rec.options, has))
    }

    /// 接続のトンネルを閉じる(接続設定を変えた、または最後のタブを閉じたとき)。
    pub(crate) async fn close_tunnel(&self, connection_id: &str) {
        self.tunnels.lock().await.remove(connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, serde_json::Value)]) -> ConnectionOptions {
        let mut o = ConnectionOptions::default();
        for (k, v) in pairs {
            o.extra.insert((*k).to_string(), v.clone());
        }
        o
    }

    #[test]
    fn no_ssh_unless_explicitly_enabled() {
        assert_eq!(SshSettings::from_options(&options(&[])).unwrap(), None);
        let o = options(&[("ssh_host", "bastion".into()), ("ssh_user", "me".into())]);
        assert_eq!(
            SshSettings::from_options(&o).unwrap(),
            None,
            "disabled by default"
        );
    }

    #[test]
    fn password_login_with_default_port() {
        let o = options(&[
            ("ssh", true.into()),
            ("ssh_host", " bastion ".into()),
            ("ssh_user", "me".into()),
        ]);
        assert_eq!(
            SshSettings::from_options(&o).unwrap(),
            Some(SshSettings {
                host: "bastion".into(),
                port: 22,
                user: "me".into(),
                login: SshLogin::Password,
            })
        );
    }

    #[test]
    fn key_login_needs_a_key_file_and_ports_are_validated() {
        let base = [
            ("ssh", true.into()),
            ("ssh_host", "b".into()),
            ("ssh_user", "u".into()),
        ];
        let with = |extra: &[(&str, serde_json::Value)]| {
            let mut all: Vec<(&str, serde_json::Value)> = base.to_vec();
            all.extend_from_slice(extra);
            SshSettings::from_options(&options(&all))
        };
        assert!(
            with(&[("ssh_auth", "key".into())]).is_err(),
            "key file required"
        );
        let ok = with(&[
            ("ssh_auth", "key".into()),
            ("ssh_key_file", "/k/id".into()),
            ("ssh_port", 2222.into()),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            (ok.port, ok.login),
            (2222, SshLogin::KeyFile("/k/id".into()))
        );
        assert!(with(&[("ssh_port", 0.into())]).is_err());
        assert!(with(&[("ssh_port", 70000.into())]).is_err());
        assert!(
            with(&[("ssh_port", "22".into())]).is_err(),
            "must be a number"
        );
    }

    #[test]
    fn host_and_user_are_required_when_enabled() {
        let o = options(&[("ssh", true.into()), ("ssh_host", "b".into())]);
        assert!(SshSettings::from_options(&o).is_err());
    }

    #[test]
    fn host_key_settings_are_per_server_and_port_and_case_insensitive() {
        assert_eq!(
            host_key_setting("Bastion.Example", 22),
            host_key_setting("bastion.example", 22)
        );
        assert_ne!(host_key_setting("b", 22), host_key_setting("b", 2222));
    }
}
