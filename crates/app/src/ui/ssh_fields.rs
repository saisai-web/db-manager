//! 接続フォームの SSH トンネルの項目。設定は `options` の `ssh_*` に、秘密は資格情報ストアに保存される。

use std::collections::BTreeMap;

use gpui_kit::component::input::{Input, InputState};
use serde_json::{Value, json};
use sqltool_core::{PasswordChange, Secret};

use super::connection_form::{ConnectionForm, text_input};
use super::prelude::*;

pub struct SshFields {
    pub enabled: bool,
    /// 鍵ファイルで認証する(しなければ、パスワード)
    key_login: bool,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    key_file: Entity<InputState>,
    /// パスワード、または鍵のパスフレーズ
    secret: Entity<InputState>,
}

fn text(extra: &BTreeMap<String, Value>, key: &str) -> String {
    extra
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

impl SshFields {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<ConnectionForm>,
        extra: &BTreeMap<String, Value>,
    ) -> Self {
        let port = extra
            .get("ssh_port")
            .and_then(Value::as_u64)
            .unwrap_or(22)
            .to_string();
        let has_secret = extra.get("ssh_secret").and_then(Value::as_bool) == Some(true);
        Self {
            enabled: extra.get("ssh").and_then(Value::as_bool) == Some(true),
            key_login: text(extra, "ssh_auth") == "key",
            host: text_input(window, cx, &text(extra, "ssh_host"), "踏み台サーバー(SSH)"),
            port: text_input(window, cx, &port, "22"),
            user: text_input(window, cx, &text(extra, "ssh_user"), "SSH のユーザー名"),
            key_file: text_input(
                window,
                cx,
                &text(extra, "ssh_key_file"),
                "秘密鍵ファイルのパス",
            ),
            secret: cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder(
                    if has_secret {
                        "(変更しない)"
                    } else {
                        "未設定"
                    }
                    .to_string(),
                )
            }),
        }
    }

    fn value(state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().trim().to_string()
    }

    /// 接続設定(`options.extra`)に書き込む。使わないときは、SSH の項目を消す。
    pub fn write_options(&self, extra: &mut BTreeMap<String, Value>, cx: &App) {
        const KEYS: [&str; 6] = [
            "ssh",
            "ssh_host",
            "ssh_port",
            "ssh_user",
            "ssh_auth",
            "ssh_key_file",
        ];
        for k in KEYS {
            extra.remove(k);
        }
        if !self.enabled {
            return;
        }
        extra.insert("ssh".into(), json!(true));
        extra.insert("ssh_host".into(), json!(Self::value(&self.host, cx)));
        // 数字でなければ、そのまま文字列で渡して、core に「正しくありません」と言わせる
        let port = Self::value(&self.port, cx);
        extra.insert(
            "ssh_port".into(),
            port.parse::<u16>()
                .map_or_else(|_| json!(port), |p| json!(p)),
        );
        extra.insert("ssh_user".into(), json!(Self::value(&self.user, cx)));
        if self.key_login {
            extra.insert("ssh_auth".into(), json!("key"));
            extra.insert(
                "ssh_key_file".into(),
                json!(Self::value(&self.key_file, cx)),
            );
        }
    }

    pub fn secret_change(&self, cx: &App) -> PasswordChange {
        let s = self.secret.read(cx).value().to_string();
        if s.is_empty() {
            PasswordChange::Keep
        } else {
            PasswordChange::Set(Secret::new(s))
        }
    }

    /// 接続先の SSH サーバー(ホスト鍵を忘れるときの宛先)。
    pub fn endpoint(&self, cx: &App) -> Option<(String, u16)> {
        let host = Self::value(&self.host, cx);
        let port = Self::value(&self.port, cx).parse().unwrap_or(22);
        (self.enabled && !host.is_empty()).then_some((host, port))
    }

    pub fn render(&self, cx: &mut Context<ConnectionForm>) -> impl IntoElement + use<> {
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            let b = Button::new(id).label(label).small();
            if on { b.primary() } else { b.ghost() }
        };
        let (enabled, key_login) = (self.enabled, self.key_login);
        let mode = div()
            .flex()
            .flex_row()
            .gap_1()
            .child(
                toggle("ssh-off", "使わない", !enabled).on_click(cx.listener(|this, _, _, cx| {
                    this.ssh.enabled = false;
                    cx.notify();
                })),
            )
            .child(
                toggle("ssh-on", "使う", enabled).on_click(cx.listener(|this, _, _, cx| {
                    this.ssh.enabled = true;
                    cx.notify();
                })),
            );
        let auth = div()
            .flex()
            .flex_row()
            .gap_1()
            .child(
                toggle("ssh-auth-password", "パスワード", !key_login).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ssh.key_login = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                toggle("ssh-auth-key", "秘密鍵", key_login).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ssh.key_login = true;
                        cx.notify();
                    },
                )),
            );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(ConnectionForm::row("SSH トンネル", mode))
            .when(enabled, |d| {
                d.child(ConnectionForm::row("SSH サーバー", Input::new(&self.host)))
                    .child(ConnectionForm::row("SSH ポート", Input::new(&self.port)))
                    .child(ConnectionForm::row("SSH ユーザー", Input::new(&self.user)))
                    .child(ConnectionForm::row("SSH 認証", auth))
                    .when(key_login, |d| {
                        d.child(ConnectionForm::row("秘密鍵", Input::new(&self.key_file)))
                    })
                    .child(ConnectionForm::row(
                        if key_login {
                            "鍵のパスフレーズ"
                        } else {
                            "SSH パスワード"
                        },
                        Input::new(&self.secret),
                    ))
                    .child(ConnectionForm::row(
                        "ホスト鍵",
                        Button::new("ssh-forget")
                            .label("登録したホスト鍵を忘れる")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((host, port)) = this.ssh.endpoint(cx) {
                                    this.model.update(cx, |m, _| m.forget_ssh_host(host, port));
                                }
                            })),
                    ))
            })
    }
}
