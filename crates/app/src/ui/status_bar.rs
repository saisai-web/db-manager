//! 画面下部のバー: 実行状況、取得行数、メモリ使用量。

use super::prelude::*;
use super::theme;
use crate::model::{AppModel, Level};

fn fmt_bytes(b: usize) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.1} MiB", b as f64 / MIB)
}

pub fn status_bar(model: &AppModel, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (text, level) = match model.active_tab() {
        Some(t) => (t.status.text.clone(), t.status.level),
        None => match &model.notice {
            Some(n) => (n.text.clone(), n.level),
            None => ("接続を選んでください".into(), Level::Info),
        },
    };
    let color = match level {
        Level::Info => theme.foreground,
        Level::Ok => theme::ok_green(),
        Level::Warn => theme::warn_amber(),
        Level::Error => theme::error_red(),
    };
    let right = model
        .active_tab()
        .and_then(|t| t.active_result())
        .map(|r| {
            let s = r.handle.stats();
            format!(
                "{} 列  /  メモリ {}  /  退避 {}",
                r.handle.columns().len(),
                fmt_bytes(s.resident_bytes),
                fmt_bytes(s.spilled_bytes as usize)
            )
        })
        .unwrap_or_default();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(26.))
        .px_3()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_xs()
        .child(div().text_color(color).child(text))
        .child(div().text_color(theme.muted_foreground).child(right))
}
