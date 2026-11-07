//! 上部のバー: 接続の選択、タブ、ER設計、Query入力、実行と停止(実行中は停止に切り替わる)、トランザクションの状態。

use sqltool_core::{RunState, TabId, TxnState};

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;

/// 描画に必要な値だけを写したもの。モデルの借用を、リスナーの登録より前に手放すため。
struct TabChip {
    id: TabId,
    title: SharedString,
    production: bool,
    selected: bool,
    uncommitted: bool,
    /// 暗号化が弱い接続の警告(証明書を検証しない、または暗号化しない)
    warning: Option<&'static str>,
}

struct Controls {
    id: TabId,
    idle: bool,
    can_run: bool,
    connected: bool,
    stopping: bool,
    txn: TxnState,
}

pub fn toolbar(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let (chips, controls) = {
        let model = ws.model.read(cx);
        let chips: Vec<TabChip> = model
            .tabs
            .iter()
            .map(|t| TabChip {
                id: t.id,
                title: t.title(),
                production: t.is_production(),
                selected: model.active == Some(t.id),
                uncommitted: t.txn.is_pending(),
                // TLS は、ネットワークで接続する DB だけの設定(SQLite のファイルには無関係)
                warning: match (t.connection.driver.as_str(), t.connection.options.tls) {
                    ("sqlite", _) | (_, sqltool_core::TlsSetting::Verify) => None,
                    (_, sqltool_core::TlsSetting::SkipVerify) => Some("⚠ 証明書を検証していません"),
                    (_, sqltool_core::TlsSetting::Disable) => Some("⚠ 暗号化されていません"),
                },
            })
            .collect();
        let controls = model.active_tab().map(|t| Controls {
            id: t.id,
            idle: t.is_idle(),
            can_run: t.is_idle() && t.connected,
            connected: t.connected,
            stopping: matches!(t.run, RunState::Cancelling),
            txn: t.txn,
        });
        (chips, controls)
    };
    let border = cx.theme().border;
    let chips: Vec<_> = chips.into_iter().map(|c| tab_chip(c, cx)).collect();

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(40.))
        .px_3()
        .border_b_1()
        .border_color(border)
        .child(
            Button::new("connections")
                .label("接続…")
                .on_click(cx.listener(|this, _, window, cx| this.open_connections(window, cx))),
        )
        .child(div().flex().flex_row().gap_1().children(chips))
        .child(div().flex_1())
        .child(
            Button::new("designer")
                .label("ER設計")
                .small()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.model
                        .update(cx, |m, cx| m.open_designer(None, window, cx));
                })),
        )
        .child(
            Button::new("query")
                .label("Query入力")
                .small()
                .disabled(controls.is_none())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.model.update(cx, |m, cx| {
                        if m.query_open {
                            m.close_query(cx)
                        } else {
                            m.open_query(window, cx)
                        }
                    });
                })),
        )
        .children(controls.map(|c| controls_view(c, cx)))
}

fn tab_chip(c: TabChip, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let id = c.id;
    let label = format!("{}{}", c.title, if c.uncommitted { " ●" } else { "" });
    let b = Button::new(("tab", id.0 as usize)).label(label).small();
    let b = if c.selected { b.primary() } else { b.ghost() };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(b.on_click(cx.listener(move |this, _, _, cx| {
            this.model.update(cx, |m, cx| m.activate(id, cx));
        })))
        .children(
            c.warning
                .map(|w| div().text_xs().text_color(theme::warn_amber()).child(w)),
        )
        .when(c.production, |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme::production_red())
                    .child("本番"),
            )
        })
        .child(
            Button::new(("close", id.0 as usize))
                .label("×")
                .ghost()
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.close_tab(id, false));
                })),
        )
}

fn controls_view(c: Controls, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let id = c.id;
    let txn = match c.txn {
        TxnState::Idle => None,
        TxnState::Open => Some(("未コミット", theme::warn_amber())),
        TxnState::Failed => Some(("中断(ROLLBACK が必要)", theme::error_red())),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .children(txn.map(|(text, color)| div().text_xs().text_color(color).child(text)))
        .when(!c.connected, |d| {
            d.child(
                Button::new("reconnect")
                    .label("再接続")
                    .small()
                    .primary()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, _| m.reconnect(id));
                    })),
            )
        })
        // 実行中は、同じ場所が停止のボタンに切り替わる
        .child(if c.idle {
            Button::new("run")
                .label("▶ 実行")
                .primary()
                .disabled(!c.can_run)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, cx| m.run(id, false, cx));
                }))
        } else {
            Button::new("stop")
                .label(if c.stopping {
                    "停止中…"
                } else {
                    "■ 停止"
                })
                .danger()
                .disabled(c.stopping)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.cancel(id));
                }))
        })
}
