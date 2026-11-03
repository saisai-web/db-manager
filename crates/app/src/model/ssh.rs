//! SSH サーバーのホスト鍵の確認。初めて接続するサーバーは、指紋を利用者に確かめてもらってから信頼する。

use gpui_kit::Context;
use sqltool_core::{Command, ConnectionId, HOST_KEY_UNKNOWN, UserError};

use super::AppModel;

/// 確認が済んだら、やり直す操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    /// 接続一覧から開こうとしていた
    Open(ConnectionId),
    /// 接続フォームでテストしていた(フォームが、自分でやり直す)
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub retry: Option<Retry>,
}

/// ホスト鍵の確認が要るという失敗か(画面には、エラーでなく確認として出す)。
pub fn is_host_key_prompt(e: &UserError) -> bool {
    e.code.as_deref() == Some(HOST_KEY_UNKNOWN)
}

impl AppModel {
    pub(super) fn host_key_unconfirmed(&mut self, host: String, port: u16, fingerprint: String) {
        self.host_key = Some(HostKeyPrompt {
            host,
            port,
            fingerprint,
            retry: self.last_attempt.clone(),
        });
    }

    /// 指紋を信頼して記録し、接続しようとしていたなら、やり直す。
    pub fn trust_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.host_key.take() else {
            return;
        };
        self.core.send(Command::TrustSshHost {
            host: p.host,
            port: p.port,
            fingerprint: p.fingerprint,
        });
        if let Some(Retry::Open(id)) = p.retry {
            self.open_tab(id);
        }
        cx.notify();
    }

    pub fn dismiss_host_key(&mut self, cx: &mut Context<Self>) {
        self.host_key = None;
        cx.notify();
    }

    pub fn forget_ssh_host(&mut self, host: String, port: u16) {
        self.core.send(Command::ForgetSshHost { host, port });
    }
}
