//! 右のパネル: CREATE 文(DB を選べる)・点検の結果・設計書の書き出し。

use gpui_kit::component::input::Input;
use sqltool_core::design::Severity;
use sqltool_core::design::ddl::Target;

use super::files::Export;
use super::{Designer, SidePanel};
use crate::ui::prelude::*;
use crate::ui::theme;

fn choice(id: impl Into<ElementId>, label: &str, active: bool) -> Button {
    let b = Button::new(id).label(label.to_string()).small();
    if active { b.primary() } else { b.ghost() }
}

pub(super) fn side_panel(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let theme_colors = cx.theme().clone();
    let issues = d.derived.issues.len();
    let tabs = div()
        .flex()
        .flex_row()
        .gap_1()
        .p_2()
        .border_b_1()
        .border_color(theme_colors.border)
        .child(
            choice("p-sql", "CREATE 文", d.panel == SidePanel::Sql)
                .on_click(cx.listener(|this, _, _, cx| this.show_panel(SidePanel::Sql, cx))),
        )
        .child(
            choice(
                "p-check",
                &format!("点検 ({issues})"),
                d.panel == SidePanel::Check,
            )
            .on_click(cx.listener(|this, _, _, cx| this.show_panel(SidePanel::Check, cx))),
        )
        .child(
            choice("p-docs", "設計書", d.panel == SidePanel::Docs)
                .on_click(cx.listener(|this, _, _, cx| this.show_panel(SidePanel::Docs, cx))),
        );
    let body = match d.panel {
        SidePanel::Sql => sql_panel(d, cx).into_any_element(),
        SidePanel::Check => check_panel(d, cx).into_any_element(),
        SidePanel::Docs => docs_panel(d, cx).into_any_element(),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(tabs)
        .child(div().flex_1().min_h_0().child(body))
}

impl Designer {
    pub(super) fn show_panel(&mut self, panel: SidePanel, cx: &mut Context<Self>) {
        self.panel = panel;
        cx.notify();
    }
}

fn sql_panel(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let theme_colors = cx.theme().clone();
    let targets: Vec<_> = Target::ALL
        .into_iter()
        .map(|t| {
            choice(("target", t as usize), t.label(), d.target == t)
                .on_click(cx.listener(move |this, _, _, cx| this.set_target(t, cx)))
        })
        .collect();
    let opts = d.options.clone();
    let toggles = [
        ("o-drop", "先に DROP する", opts.drop_first),
        ("o-fkidx", "外部キーに索引", opts.index_foreign_keys),
        ("o-comment", "論理名をコメントに", opts.comments),
    ];
    let toggle_buttons: Vec<_> = toggles
        .into_iter()
        .enumerate()
        .map(|(i, (id, label, active))| {
            choice(id, label, active).on_click(cx.listener(move |this, _, _, cx| {
                match i {
                    0 => this.options.drop_first = !this.options.drop_first,
                    1 => this.options.index_foreign_keys = !this.options.index_foreign_keys,
                    _ => this.options.comments = !this.options.comments,
                }
                this.refresh();
                cx.notify();
            }))
        })
        .collect();
    let warnings: Vec<_> = d
        .derived
        .ddl
        .warnings
        .iter()
        .map(|w| {
            div()
                .text_xs()
                .text_color(theme::warn_amber())
                .child(format!("⚠ {w}"))
        })
        .collect();
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .children(targets),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .children(toggle_buttons),
        )
        .child(Input::new(&d.schema_input))
        .when(!warnings.is_empty(), |x| {
            x.child(
                div()
                    .max_h(px(90.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(warnings),
            )
        })
        .child(
            div()
                .id("ddl-scroll")
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .p_2()
                .rounded_md()
                .bg(theme_colors.muted)
                .font_family(theme::mono_font())
                .text_size(px(12.))
                .whitespace_nowrap()
                .child(d.derived.preview.clone()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_1()
                .child(
                    Button::new("sql-copy")
                        .label("コピー")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.copy_sql(cx))),
                )
                .child(
                    Button::new("sql-save")
                        .label("SQL を保存…")
                        .small()
                        .on_click(cx.listener(|this, _, w, cx| this.export(Export::Sql, w, cx))),
                )
                .child(
                    Button::new("sql-editor")
                        .label("SQL エディタへ")
                        .small()
                        .primary()
                        .on_click(cx.listener(|this, _, w, cx| this.send_to_editor(w, cx))),
                ),
        )
}

fn check_panel(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    let rows: Vec<_> = d
        .derived
        .issues
        .iter()
        .enumerate()
        .map(|(i, issue)| {
            let (label, color) = match issue.severity {
                Severity::Error => ("エラー", theme::error_red()),
                Severity::Warning => ("警告", theme::warn_amber()),
            };
            let table = issue.table.clone();
            div()
                .id(("issue", i))
                .flex()
                .flex_row()
                .gap_2()
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|h| h.bg(cx.theme().accent))
                .text_sm()
                .child(div().text_color(color).w(px(40.)).child(label))
                .child(div().text_color(muted).child(table.clone()))
                .child(div().flex_1().child(issue.message.clone()))
                .on_click(cx.listener(move |this, _, w, cx| {
                    if !table.is_empty() {
                        this.select(&table, w, cx);
                    }
                }))
        })
        .collect();
    div()
        .size_full()
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_hidden()
        .when(rows.is_empty(), |x| {
            x.child(
                div()
                    .text_color(theme::ok_green())
                    .child("問題は見つかりませんでした"),
            )
        })
        .children(rows)
}

fn docs_panel(d: &Designer, cx: &mut Context<Designer>) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    let (target, appendix) = (d.target.label(), d.appendix_ddl);
    div()
        .size_full()
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_sm().child(
            "表・列・関係・制約・索引の定義書を、表紙・目次・一覧つきで書き出します。\
             論理名・備考・多重度も載ります。",
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    Button::new("doc-xlsx")
                        .label("Excel (.xlsx)")
                        .primary()
                        .on_click(cx.listener(|this, _, w, cx| this.export(Export::Xlsx, w, cx))),
                )
                .child(
                    Button::new("doc-html")
                        .label("HTML")
                        .primary()
                        .on_click(cx.listener(|this, _, w, cx| this.export(Export::Html, w, cx))),
                )
                .child(
                    Button::new("doc-svg")
                        .label("ER 図 (SVG)")
                        .on_click(cx.listener(|this, _, w, cx| this.export(Export::Svg, w, cx))),
                ),
        )
        .child(
            choice("doc-appendix", &format!("付録に CREATE 文({target})を付ける"), appendix)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.appendix_ddl = !this.appendix_ddl;
                    cx.notify();
                })),
        )
        .child(
            div().text_xs().text_color(muted).child(
                "表紙の題名・作成者・バージョン・概要は、テキストの先頭の @title @author @version \
                 @description で決めます。ER 図の名前は、上の「物理名 / 論理名」の表示に合わせます。",
            ),
        )
}
