//! 接続先の編集フォーム。テスト、保存ができる。パスワードは OS の資格情報ストアに保存される。

use super::prelude::*;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputState};
use sqltool_core::{
    ConnectionInput, ConnectionOptions, ConnectionSummary, EnvLabel, PasswordChange, Secret,
    TlsSetting,
};

use super::theme;
use crate::model::{AppModel, Retry};

pub struct ConnectionForm {
    pub(super) model: Entity<AppModel>,
    id: Option<String>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    database: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    env: EnvLabel,
    /// `postgresql` か `sqlite`
    driver: &'static str,
    tls: TlsSetting,
    /// 検証に使う CA 証明書(PEM)のパス
    ca_file: Entity<InputState>,
    pub(super) ssh: super::ssh_fields::SshFields,
    /// 既存の接続が持つ、この画面で扱わない設定(新しい版の項目など)。保存で消さない
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

pub(super) fn text_input(
    window: &mut Window,
    cx: &mut Context<ConnectionForm>,
    value: &str,
    hint: &str,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(hint.to_string())
            .default_value(value.to_string())
    })
}

impl ConnectionForm {
    pub fn new(
        model: Entity<AppModel>,
        existing: Option<&ConnectionSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let c = existing;
        let get = |f: fn(&ConnectionSummary) -> Option<String>| c.and_then(f).unwrap_or_default();
        let port = c
            .and_then(|c| c.port)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "5432".into());
        let has_password = c.is_some_and(|c| c.has_password);

        Self {
            id: c.map(|c| c.id.clone()),
            driver: if c.is_some_and(|c| c.driver == "sqlite") {
                "sqlite"
            } else {
                "postgresql"
            },
            name: text_input(window, cx, c.map_or("", |c| c.name.as_str()), "接続名"),
            host: text_input(window, cx, &get(|c| c.host.clone()), "localhost"),
            port: text_input(window, cx, &port, "5432"),
            database: text_input(window, cx, &get(|c| c.database.clone()), "データベース名"),
            user: text_input(window, cx, &get(|c| c.username.clone()), "ユーザー名"),
            password: cx.new(|cx| {
                InputState::new(window, cx).masked(true).placeholder(
                    if has_password {
                        "(変更しない)"
                    } else {
                        "パスワード"
                    }
                    .to_string(),
                )
            }),
            env: c.map_or(EnvLabel::Dev, |c| c.env_label),
            // TLS はフェーズ 2。対応するまでは「暗号化しない」を明示して接続する
            tls: c.map_or(TlsSetting::Verify, |c| c.options.tls),
            ca_file: text_input(
                window,
                cx,
                c.and_then(|c| c.options.extra.get("tls_ca_file"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                "CA 証明書(PEM)のパス。省略可",
            ),
            ssh: super::ssh_fields::SshFields::new(
                window,
                cx,
                &c.map(|c| c.options.extra.clone()).unwrap_or_default(),
            ),
            extra: c.map(|c| c.options.extra.clone()).unwrap_or_default(),
            model,
        }
    }

    fn value(state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().trim().to_string()
    }

    fn to_input(&self, cx: &App) -> ConnectionInput {
        let pw = self.password.read(cx).value().to_string();
        ConnectionInput {
            id: self.id.clone(),
            name: Self::value(&self.name, cx),
            driver: self.driver.into(),
            host: Some(Self::value(&self.host, cx)).filter(|s| !s.is_empty()),
            port: Self::value(&self.port, cx).parse().ok(),
            database: Some(Self::value(&self.database, cx)).filter(|s| !s.is_empty()),
            username: Some(Self::value(&self.user, cx)).filter(|s| !s.is_empty()),
            options: self.options(cx),
            env_label: self.env,
            password: if pw.is_empty() {
                PasswordChange::Keep
            } else {
                PasswordChange::Set(Secret::new(pw))
            },
            ssh_secret: self.ssh.secret_change(cx),
        }
    }

    fn options(&self, cx: &App) -> ConnectionOptions {
        let mut extra = self.extra.clone();
        match Self::value(&self.ca_file, cx) {
            ca if ca.is_empty() => extra.remove("tls_ca_file"),
            ca => extra.insert("tls_ca_file".into(), serde_json::Value::String(ca)),
        };
        self.ssh.write_options(&mut extra, cx);
        ConnectionOptions {
            tls: self.tls,
            extra,
            ..Default::default()
        }
    }

    pub(super) fn row(label: &str, field: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(div().w(px(110.)).text_sm().child(label.to_string()))
            .child(div().flex_1().child(field))
    }
}

impl Render for ConnectionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _theme = cx.theme().clone();
        let env_button = |id: &'static str,
                          label: &'static str,
                          value: EnvLabel,
                          form: &Self,
                          cx: &mut Context<Self>| {
            let b = Button::new(id).label(label).small();
            let b = if form.env == value {
                if value == EnvLabel::Production {
                    b.danger()
                } else {
                    b.primary()
                }
            } else {
                b.ghost()
            };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.env = value;
                cx.notify();
            }))
        };
        let result = self.model.read(cx).test_result.clone();
        let result_line = result.map(|r| match r {
            Ok(v) => div().text_sm().text_color(theme::ok_green()).child(format!(
                "接続できました: {}",
                v.split(" on ").next().unwrap_or("")
            )),
            Err(e) => div()
                .text_sm()
                .text_color(theme::error_red())
                .child(format!("接続できません: {e}")),
        });
        // テスト中に、初めて接続する SSH サーバーが見つかった: 指紋を見せ、信頼するなら、テストをやり直す
        let host_key = self
            .model
            .read(cx)
            .host_key
            .clone()
            .filter(|p| p.retry == Some(Retry::Test))
            .map(|p| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(_theme.muted)
                    .child(div().text_sm().text_color(theme::warn_amber()).child(format!(
                        "初めて接続する SSH サーバー {}:{} です。ホスト鍵の指紋が、管理者に聞いたものと一致するときだけ信頼してください",
                        p.host, p.port
                    )))
                    .child(
                        div()
                            .text_sm()
                            .font_family(theme::mono_font())
                            .child(p.fingerprint),
                    )
                    .child(
                        div().flex().flex_row().gap_2().child(
                            Button::new("host-key-trust-test")
                                .label("信頼して、もう一度テスト")
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let input = this.to_input(cx);
                                    this.model.update(cx, |m, cx| {
                                        m.trust_host_key(cx);
                                        m.test_connection(input, cx);
                                    });
                                })),
                        ),
                    )
            });
        let dev = env_button("env-dev", "開発", EnvLabel::Dev, self, cx);
        let stg = env_button("env-stg", "検証", EnvLabel::Staging, self, cx);
        let prod = env_button("env-prod", "本番", EnvLabel::Production, self, cx);
        let tls = self.tls;
        let pg = self.driver == "postgresql";
        let driver_button =
            |id: &'static str, label: &'static str, value: &'static str, cx: &mut Context<Self>| {
                let b = Button::new(id).label(label).small();
                let b = if self.driver == value {
                    b.primary()
                } else {
                    b.ghost()
                };
                b.on_click(cx.listener(move |this, _, _, cx| {
                    this.driver = value;
                    cx.notify();
                }))
            };
        let driver_buttons = div()
            .flex()
            .flex_row()
            .gap_1()
            .child(driver_button("driver-pg", "PostgreSQL", "postgresql", cx))
            .child(driver_button("driver-sqlite", "SQLite", "sqlite", cx));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::row("接続名", Input::new(&self.name)))
            .child(Self::row("種類", driver_buttons))
            .child(Self::row(if pg { "ホスト" } else { "ファイル" }, Input::new(&self.host)))
            .when(pg, |d| {
                d.child(Self::row("ポート", Input::new(&self.port)))
                    .child(Self::row("データベース", Input::new(&self.database)))
                    .child(Self::row("ユーザー", Input::new(&self.user)))
                    .child(Self::row("パスワード", Input::new(&self.password)))
            })
            .child(Self::row(
                "環境",
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(dev)
                    .child(stg)
                    .child(prod),
            ))
            .when(self.env == EnvLabel::Production, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme::production_red())
                        .child("本番ラベル: 画面が赤枠になり、接続直後は読み取り専用になります"),
                )
            })
            .when(pg, |d| d.child(Self::row(
                "暗号化",
                Button::new("tls")
                    .label(match tls {
                        TlsSetting::Verify => "TLS で暗号化(証明書を検証)",
                        TlsSetting::SkipVerify => "TLS で暗号化(証明書を検証しない)",
                        TlsSetting::Disable => "暗号化しない",
                    })
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        // 検証あり → 検証なし → 暗号化しない → 検証あり
                        this.tls = match this.tls {
                            TlsSetting::Verify => TlsSetting::SkipVerify,
                            TlsSetting::SkipVerify => TlsSetting::Disable,
                            TlsSetting::Disable => TlsSetting::Verify,
                        };
                        cx.notify();
                    })),
            )))
            .when(pg && tls == TlsSetting::Verify, |d| {
                d.child(Self::row("CA 証明書", Input::new(&self.ca_file)))
            })
            .when(pg && tls != TlsSetting::Verify, |d| {
                let (text, color) = if tls == TlsSetting::SkipVerify {
                    ("警告: 接続先が本物かを確かめません。通信を盗み見る第三者に気づけません", theme::warn_amber())
                } else {
                    ("警告: 通信は暗号化されません。パスワードや結果が、ネットワーク上で読み取られます", theme::error_red())
                };
                d.child(div().text_xs().text_color(color).child(text))
            })
            .when(pg, |d| d.child(self.ssh.render(cx)))
            .children(host_key)
            .children(result_line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("form-test")
                            .label("接続テスト")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let input = this.to_input(cx);
                                this.model.update(cx, |m, cx| m.test_connection(input, cx));
                            })),
                    )
                    .child(
                        Button::new("form-save")
                            .label("保存")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let input = this.to_input(cx);
                                this.model.update(cx, |m, _| m.save_connection(input));
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("form-cancel")
                            .label("キャンセル")
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}
