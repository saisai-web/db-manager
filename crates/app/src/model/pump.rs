//! core のイベントを、画面のスレッドでモデルに適用する。

use gpui_kit::{AnyWindowHandle, App, Entity};
use sqltool_core::EventStream;

use super::AppModel;

pub fn start_pump(
    model: &Entity<AppModel>,
    mut events: EventStream,
    window: AnyWindowHandle,
    cx: &mut App,
) {
    let model = model.downgrade();
    cx.spawn(async move |cx| {
        // tokio のチャネルは、tokio のランタイム外(GPUI の executor)でも待てる
        while let Some(event) = events.recv().await {
            let Some(model) = model.upgrade() else { break };
            let applied = window.update(cx, |_, window, cx| {
                model.update(cx, |m, cx| m.apply(event, window, cx))
            });
            if applied.is_err() {
                break; // ウィンドウが閉じられた
            }
        }
    })
    .detach();
}
