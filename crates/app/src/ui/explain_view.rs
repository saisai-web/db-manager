//! 実行計画の可視化。ノードを字下げした木で描き、重いノードほど長い棒で示す。

use sqltool_core::{Plan, PlanNode, TabId};

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;

/// 描く行(木を平らにしたもの)。
struct Line {
    depth: usize,
    title: String,
    detail: String,
    conditions: Vec<String>,
    /// 全ノードの中での、このノード自身の重さの割合(0〜1)
    weight: f32,
    heaviest: bool,
}

fn weight_of(n: &PlanNode) -> f64 {
    n.self_ms().unwrap_or_else(|| n.self_cost())
}

fn lines(plan: &Plan) -> Vec<Line> {
    let mut all = Vec::new();
    plan.root.walk(&mut all);
    let max = all
        .iter()
        .map(|n| weight_of(n))
        .fold(0.0_f64, f64::max)
        .max(f64::MIN_POSITIVE);
    let heaviest = plan.heaviest() as *const PlanNode;
    let mut out = Vec::new();
    fn visit(n: &PlanNode, depth: usize, max: f64, heaviest: *const PlanNode, out: &mut Vec<Line>) {
        let time = n
            .self_ms()
            .map(|t| format!("  {t:.3} ms"))
            .unwrap_or_default();
        let actual = n
            .actual_rows
            .map(|r| format!(" / 実際 {r:.0}"))
            .unwrap_or_default();
        out.push(Line {
            depth,
            title: match &n.target {
                Some(t) => format!("{} on {t}", n.kind),
                None => n.kind.clone(),
            },
            detail: format!(
                "コスト {:.2}..{:.2}  行 {:.0}{actual}{time}",
                n.startup_cost, n.total_cost, n.rows
            ),
            conditions: n
                .conditions
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect(),
            weight: (weight_of(n) / max) as f32,
            heaviest: std::ptr::eq(n, heaviest),
        });
        for c in &n.children {
            visit(c, depth + 1, max, heaviest, out);
        }
    }
    visit(&plan.root, 0, max, heaviest, &mut out);
    out
}

pub fn explain_overlay(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    let model = ws.model.read(cx);
    let tab = model.active_tab()?;
    let e = tab.explain.as_ref()?;
    let tab_id: TabId = tab.id;
    let (sql, analyze, summary) = (
        e.sql.clone(),
        e.analyze,
        format!(
            "計画 {}  実行 {}",
            e.plan
                .planning_ms
                .map_or("—".into(), |t| format!("{t:.3} ms")),
            e.plan
                .execution_ms
                .map_or("—".into(), |t| format!("{t:.3} ms"))
        ),
    );
    let rows: Vec<AnyElement> = lines(&e.plan)
        .into_iter()
        .map(|l| {
            let bar = if l.heaviest {
                theme::error_red()
            } else {
                theme_colors.primary.opacity(0.5)
            };
            div()
                .flex()
                .flex_col()
                .pl(px(16. * l.depth as f32))
                .py_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(120.))
                                .h(px(8.))
                                .rounded_sm()
                                .bg(theme_colors.muted)
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_sm()
                                        .bg(bar)
                                        .w(px(120. * l.weight.clamp(0.02, 1.0))),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .when(l.heaviest, |d| d.text_color(theme::error_red()))
                                .child(l.title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme_colors.muted_foreground)
                                .child(l.detail),
                        ),
                )
                .children(l.conditions.into_iter().map(|c| {
                    div()
                        .pl(px(130.))
                        .text_xs()
                        .font_family(theme::mono_font())
                        .text_color(theme_colors.muted_foreground)
                        .child(c)
                }))
                .into_any_element()
        })
        .collect();
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .child(
                div()
                    .w(px(880.))
                    .h(px(600.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme_colors.border)
                    .bg(theme_colors.background)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if analyze {
                                "実行計画(実測つき)"
                            } else {
                                "実行計画"
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme::mono_font())
                            .text_color(theme_colors.muted_foreground)
                            .child(sqltool_core::summarize(&sql)),
                    )
                    .child(div().text_xs().child(summary))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme_colors.muted_foreground)
                            .child(
                                "棒は、そのノード自身の重さ(子を除く)。赤が最も重いノードです。",
                            ),
                    )
                    .child(
                        div()
                            .id("explain-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(rows),
                    )
                    .child(
                        div().flex().flex_row().justify_end().child(
                            Button::new("explain-close")
                                .label("閉じる")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.update(cx, |m, cx| m.close_explain(tab_id, cx));
                                })),
                        ),
                    ),
            ),
    )
}
