//! ER 図。表を箱で、外部キーを線で描く。配置は core が計算する(参照される表が左、参照する表が右)。

use sqltool_core::{
    BOX_WIDTH, ErGraph, ErRelation, GAP_X, HEADER_HEIGHT, MAX_COLUMNS_SHOWN, ROW_HEIGHT, TabId,
    box_height, column_anchor, layout_er,
};

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;
use crate::model::graph_of;

const MARGIN: f32 = 20.;

/// 線の 1 区間(左上と大きさ)。
type Segment = (f32, f32, f32, f32);

/// 外部キー 1 本を、横 → 縦 → 横の折れ線にする。参照先が同じ層か右にある(循環や自己参照)なら描かない。
fn route(g: &ErGraph, positions: &[(f32, f32)], r: &ErRelation, lane: usize) -> Vec<Segment> {
    let (from, to) = (&g.tables[r.from], &g.tables[r.to]);
    let (fx, fy) = positions[r.from];
    let (tx, ty) = positions[r.to];
    let start_x = fx;
    let end_x = tx + BOX_WIDTH;
    if start_x <= end_x {
        return Vec::new();
    }
    let y0 = fy
        + r.from_columns
            .first()
            .map_or(HEADER_HEIGHT / 2., |c| column_anchor(from, c));
    let y1 = ty
        + r.to_columns
            .first()
            .map_or(HEADER_HEIGHT / 2., |c| column_anchor(to, c));
    // 同じ隙間を通る線が重ならないよう、少しずつずらす
    let mid = end_x + GAP_X / 2. + (lane % 6) as f32 * 4. - 10.;
    vec![
        (end_x, y1, mid - end_x, 1.),
        (mid, y0.min(y1), 1., (y0 - y1).abs() + 1.),
        (mid, y0, start_x - mid, 1.),
    ]
}

fn table_box(g: &ErGraph, ix: usize, pos: (f32, f32), cx: &Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme().clone();
    let t = &g.tables[ix];
    let hidden = t.columns.len().saturating_sub(MAX_COLUMNS_SHOWN);
    let rows = t.columns.iter().take(MAX_COLUMNS_SHOWN).map(|c| {
        div()
            .h(px(ROW_HEIGHT))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_xs()
            .child(div().w(px(18.)).text_color(theme.muted_foreground).child(
                match (c.primary, c.foreign) {
                    (true, _) => "PK",
                    (_, true) => "FK",
                    _ => "",
                },
            ))
            .child(
                div()
                    .when(c.primary, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(c.name.clone()),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(c.type_name.clone()),
            )
    });
    div()
        .absolute()
        .left(px(pos.0 + MARGIN))
        .top(px(pos.1 + MARGIN))
        .w(px(BOX_WIDTH))
        .h(px(box_height(t)))
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .overflow_hidden()
        .child(
            div()
                .h(px(HEADER_HEIGHT))
                .px_2()
                .flex()
                .items_center()
                .bg(theme.primary.opacity(0.15))
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t.name.clone()),
        )
        .children(rows)
        .when(hidden > 0, |d| {
            d.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("… ほか {hidden} 列")),
            )
        })
}

pub fn er_overlay(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<impl IntoElement + use<>> {
    let theme_colors = cx.theme().clone();
    let model = ws.model.read(cx);
    let tab = model.active_tab()?;
    let schema = tab.er.clone()?;
    let tab_id: TabId = tab.id;
    let (graph, progress) = graph_of(model.schemas.get(&tab.connection.id)?, &schema);
    let layout = layout_er(&graph);
    let line_color = theme_colors.muted_foreground;

    let lines: Vec<AnyElement> = graph
        .relations
        .iter()
        .enumerate()
        .flat_map(|(lane, r)| route(&graph, &layout.positions, r, lane))
        .map(|(x, y, w, h)| {
            div()
                .absolute()
                .left(px(x + MARGIN))
                .top(px(y + MARGIN))
                .w(px(w))
                .h(px(h))
                .bg(line_color)
                .into_any_element()
        })
        .collect();
    let boxes: Vec<AnyElement> = (0..graph.tables.len())
        .map(|i| table_box(&graph, i, layout.positions[i], cx).into_any_element())
        .collect();

    let note = if progress.loaded < progress.total {
        format!("読み込み中… {}/{} 表", progress.loaded, progress.total)
    } else {
        format!(
            "{} 表 / 外部キー {} 本(線は、参照する表 → 参照される表。左が参照される側)",
            graph.tables.len(),
            graph.relations.len()
        )
    };
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
                    .w(px(1000.))
                    .h(px(640.))
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
                            .child(format!("ER 図  {schema}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme_colors.muted_foreground)
                            .child(note),
                    )
                    .when(graph.tables.is_empty() && progress.total == 0, |d| {
                        d.child(div().text_color(theme::error_red()).child("表がありません"))
                    })
                    .child(
                        div()
                            .id("er-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .child(
                                div()
                                    .relative()
                                    .w(px(layout.size.0 + 2. * MARGIN))
                                    .h(px(layout.size.1 + 2. * MARGIN))
                                    .children(lines)
                                    .children(boxes),
                            ),
                    )
                    .child(
                        div().flex().flex_row().justify_end().child(
                            Button::new("er-close")
                                .label("閉じる")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.update(cx, |m, cx| m.close_er(tab_id, cx));
                                })),
                        ),
                    ),
            ),
    )
}
