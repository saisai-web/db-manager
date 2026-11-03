//! 結果タブと結果グリッド。取得上限で止まっているときは、続きの取得と打ち切りを選べる。

use gpui_kit::component::table::{DataTable, TableState};
use sqltool_core::{ResultId, TabId};

use super::edit_bar::{edit_bar, snapshot as edit_snapshot};
use super::export_bar::export_buttons;
use super::grid::GridDelegate;
use super::prelude::*;
use super::workspace::Workspace;

/// 描画に必要な値だけを写したもの(モデルの借用を、リスナーの登録より前に手放すため)。
struct Snapshot {
    tab: TabId,
    labels: Vec<String>,
    active: usize,
    result: ResultId,
    paused: bool,
    rows: u64,
    table: Entity<TableState<GridDelegate>>,
}

fn snapshot(ws: &Workspace, cx: &App) -> Option<Snapshot> {
    let tab = ws.model.read(cx).active_tab()?;
    let active = tab.active_result()?;
    Some(Snapshot {
        tab: tab.id,
        labels: tab
            .results
            .iter()
            .enumerate()
            .map(|(i, r)| format!("結果 {} ({} 行)", i + 1, r.rows))
            .collect(),
        active: tab.active_result,
        result: active.handle.id(),
        paused: active.paused,
        rows: active.rows,
        table: active.table.clone(),
    })
}

pub fn results_pane(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let theme = cx.theme().clone();
    let Some(s) = snapshot(ws, cx) else {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.muted_foreground)
            .child("結果はここに表示されます")
            .into_any_element();
    };
    let result = s.result;
    let bar = edit_snapshot(ws, cx).map(|b| edit_bar(b, cx));
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(result_tabs(&s, cx))
        .children(bar)
        .when(s.paused, |d| d.child(paused_bar(s.rows, result, cx)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .child(DataTable::new(&s.table).stripe(true).bordered(false)),
        )
        .into_any_element()
}

fn paused_bar(
    rows: u64,
    result: ResultId,
    cx: &mut Context<Workspace>,
) -> impl IntoElement + use<> {
    let warning = cx.theme().warning;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(34.))
        .bg(warning.opacity(0.15))
        .child(div().text_sm().child(format!(
            "{rows} 行で受信を一時停止しています(接続は占有されたままです)"
        )))
        .child(
            Button::new("fetch-next")
                .label("続きを取得")
                .small()
                .primary()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.fetch_more(result, false));
                })),
        )
        .child(
            Button::new("fetch-rest")
                .label("残りをすべて取得")
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.fetch_more(result, true));
                })),
        )
        .child(
            Button::new("fetch-stop")
                .label("打ち切り")
                .small()
                .danger()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.update(cx, |m, _| m.stop_fetch(result));
                })),
        )
}

fn result_tabs(s: &Snapshot, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let tab = s.tab;
    let chips: Vec<_> = s
        .labels
        .iter()
        .enumerate()
        .map(|(ix, label)| {
            let b = Button::new(("result-tab", ix)).small().label(label.clone());
            let b = if ix == s.active {
                b.primary()
            } else {
                b.ghost()
            };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.model.update(cx, |m, cx| m.select_result(tab, ix, cx));
            }))
        })
        .collect();
    div()
        .flex()
        .flex_row()
        .gap_1()
        .items_center()
        .h(px(32.))
        .px_2()
        .border_b_1()
        .border_color(border)
        .children(chips)
        .child(div().flex_1())
        .child(export_buttons(tab, s.result, cx))
}
