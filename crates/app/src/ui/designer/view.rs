//! ER 設計の画面全体の組み立て。

use gpui_kit::component::input::Editor;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use sqltool_core::design::Severity;

use super::bar::toolbar;
use super::detail_view::detail_dialog;
use super::diagram::diagram;
use super::dialog::{export_dialog, modal};
use super::palette::rail;
use super::relation_view::relation_dialog;
use super::type_menu::type_panel;
use super::{Designer, SidePanel};
use crate::ui::prelude::*;
use crate::ui::theme;

impl Render for Designer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_colors = cx.theme().clone();
        let text_pane = div()
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme_colors.border)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(theme::mono_font())
                    .text_size(px(13.))
                    .child(Editor::new(&self.text).h_full().appearance(false)),
            )
            .when_some(self.error.clone(), |d, e| {
                d.child(
                    div()
                        .p_2()
                        .text_xs()
                        .text_color(theme::error_red())
                        .border_t_1()
                        .border_color(theme_colors.border)
                        .child(e),
                )
            });
        // 図は画面いっぱいに出す。テキストは、開いたときだけ左に並べる(境界をドラッグして幅を変えられる)
        let canvas = diagram(self, window, cx);
        let main = if self.show_text {
            div().flex_1().min_w_0().child(
                h_resizable("designer-panes")
                    .child(resizable_panel().size(px(360.)).child(text_pane))
                    .child(resizable_panel().child(canvas)),
            )
        } else {
            div().flex_1().min_w_0().child(canvas)
        };
        // 左端のレール: ワンクリックで、左にテキスト記法を開閉する
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(rail(self, cx))
            .child(main);

        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .bg(theme_colors.background)
            .child(toolbar(self, cx))
            .child(body)
            .child(status(self, cx))
            .when(self.export_open, |d| {
                d.child(export_dialog(self, window, cx))
            })
            .children(detail_dialog(self, window, cx))
            .children(relation_dialog(self, window, cx))
            .children(
                self.type_menu
                    .as_ref()
                    .filter(|m| m.in_dialog)
                    .and_then(|_| type_panel(self, cx))
                    .map(|panel| {
                        modal(
                            "type-dialog",
                            "型を選ぶ".into(),
                            (360.0, Some(480.0)),
                            window,
                            div()
                                .id("type-scroll")
                                .size_full()
                                .overflow_y_scroll()
                                .child(panel),
                            |this, _, cx| this.close_type_menu(cx),
                            cx,
                        )
                    }),
            )
    }
}

fn status(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let theme_colors = cx.theme().clone();
    let errors = d
        .derived
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    let warnings = d.derived.issues.len() - errors;
    let relations: usize = d.design.tables.iter().map(|t| t.foreign_keys.len()).sum();
    let (is_error, text) = d.message.clone().unwrap_or((
        false,
        "列名・型をクリックで編集 / 見出しの「＋」で列を追加 / ダブルクリックで詳細(関係線も) / 右下の角で表の大きさ / ⌘+スクロールで拡大縮小"
            .to_string(),
    ));
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .h(px(26.))
        .px_3()
        .border_t_1()
        .border_color(theme_colors.border)
        .text_xs()
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(if is_error {
                    theme::error_red()
                } else {
                    theme_colors.muted_foreground
                })
                .child(text),
        )
        .child(format!(
            "{} 表 / 外部キー {relations} 本",
            d.design.tables.len()
        ))
        .when(errors > 0, |x| {
            x.child(
                div()
                    .id("status-errors")
                    .cursor_pointer()
                    .text_color(theme::error_red())
                    .on_click(
                        cx.listener(|this, _, w, cx| this.open_export(SidePanel::Check, w, cx)),
                    )
                    .child(format!("エラー {errors}")),
            )
        })
        .when(warnings > 0, |x| {
            x.child(
                div()
                    .id("status-warnings")
                    .cursor_pointer()
                    .text_color(theme::warn_amber())
                    .on_click(
                        cx.listener(|this, _, w, cx| this.open_export(SidePanel::Check, w, cx)),
                    )
                    .child(format!("警告 {warnings}")),
            )
        })
}
