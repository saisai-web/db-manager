//! 画面全体の配置: 上にツールバー、左にスキーマツリー、右に結果、下にステータスバー。
//! Query入力を開くと、結果の上に SQL を書く窓が並ぶ。
//! 本番ラベルの接続では、画面全体に赤い枠を表示する。

use super::prelude::*;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};

use super::confirm::confirm_overlay;
use super::connections::open_list;
use super::edit_preview::edit_preview_overlay;
use super::er_view::er_overlay;
use super::host_key_view::host_key_overlay;
use super::import_preview::import_preview_overlay;
use super::query_pane::query_pane;
use super::results_pane::results_pane;
use super::schema_tree::schema_tree;
use super::status_bar::status_bar;
use super::theme;
use super::toolbar::toolbar;
use crate::actions::{CONTEXT, CancelRun, OpenConnections, RunAtCursor};
use crate::model::AppModel;

pub struct Workspace {
    pub model: Entity<AppModel>,
}

impl Workspace {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        // モデルが変わったら描画し直す
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
    }
}

impl Workspace {
    fn active_tab_id(&self, cx: &App) -> Option<sqltool_core::TabId> {
        self.model.read(cx).active
    }

    fn on_run(&mut self, all: bool, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_id(cx) {
            self.model.update(cx, |m, cx| m.run(tab, all, cx));
        }
    }

    fn on_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_id(cx) {
            self.model.update(cx, |m, _| m.cancel(tab));
        }
    }

    pub fn open_connections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        open_list(self.model.clone(), window, cx);
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().background;
        let production = self
            .model
            .read(cx)
            .active_tab()
            .is_some_and(|t| t.is_production());
        let status = status_bar(self.model.read(cx), cx);
        let toolbar = toolbar(self, cx);
        let tree = schema_tree(self, cx);
        let results = results_pane(self, cx);
        let overlay = confirm_overlay(self, cx);
        let preview = edit_preview_overlay(self, cx);
        let import = import_preview_overlay(self, cx);
        let query = query_pane(self, cx);
        let er = er_overlay(self, cx);
        let host_key = host_key_overlay(self, cx);
        let designer = self
            .model
            .read(cx)
            .designer
            .clone()
            .filter(|d| d.read(cx).visible);

        let body = h_resizable("main")
            .child(resizable_panel().size(px(280.)).child(tree))
            .child(
                resizable_panel().child(match query {
                    // Query入力を開いているときは、結果の上に並べる
                    Some(query) => v_resizable("right")
                        .child(resizable_panel().size(px(260.)).child(query))
                        .child(resizable_panel().child(results))
                        .into_any_element(),
                    None => results.into_any_element(),
                }),
            );

        div()
            .size_full()
            .relative()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &RunAtCursor, _, cx| this.on_run(false, cx)))
            .on_action(cx.listener(|this, _: &CancelRun, _, cx| this.on_cancel(cx)))
            .on_action(cx.listener(|this, _: &OpenConnections, window, cx| {
                this.open_connections(window, cx)
            }))
            .bg(background)
            .flex()
            .flex_col()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
            .child(status)
            .children(overlay)
            .children(preview)
            .children(import)
            .children(er)
            .children(host_key)
            .children(designer)
            // 本番ラベル: 画面全体を赤い枠で囲む(操作は邪魔しない)
            .when(production, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_4()
                        .border_color(theme::production_red()),
                )
            })
    }
}
