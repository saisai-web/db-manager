//! ER 図の描画と操作。表は箱(見出しをドラッグして移動、列をクリックして関係をつくる)、関係線は 1 枚の canvas に描く。

use std::f32::consts::TAU;

use sqltool_core::design::diagram::{Shape, marker_shapes};

use super::Designer;
use super::relation::edge_hits;
use super::table_box::table_box;
use super::type_menu::type_popup;
use crate::ui::prelude::*;

pub(super) const MARGIN: f32 = 24.0;

/// 関係線と、その端の印を描く。
fn paint_edges(
    bounds: Bounds<Pixels>,
    lines: &[Vec<(f32, f32)>],
    shapes: &[Shape],
    color: Hsla,
    background: Hsla,
    window: &mut Window,
) {
    let o = bounds.origin;
    let at = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
    let mut stroke = PathBuilder::stroke(px(1.3));
    let mut fill = PathBuilder::fill();
    let mut rings = PathBuilder::stroke(px(1.3));
    for l in lines {
        if let Some(first) = l.first() {
            stroke.move_to(at(first.0, first.1));
            for p in &l[1..] {
                stroke.line_to(at(p.0, p.1));
            }
        }
    }
    for s in shapes {
        match *s {
            Shape::Line(x1, y1, x2, y2) => {
                stroke.move_to(at(x1, y1));
                stroke.line_to(at(x2, y2));
            }
            Shape::Circle(cx, cy, r) => {
                let pts: Vec<_> = (0..16)
                    .map(|i| {
                        let a = i as f32 * TAU / 16.0;
                        at(cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect();
                fill.add_polygon(&pts, true);
                rings.add_polygon(&pts, true);
            }
        }
    }
    // 線 → 円の内側(線を隠す)→ 円の縁、の順
    for (builder, c) in [(stroke, color), (fill, background), (rings, color)] {
        if let Ok(path) = builder.build() {
            window.paint_path(path, c);
        }
    }
}

fn scaled(s: Shape, z: f32) -> Shape {
    match s {
        Shape::Line(a, b, c, d) => Shape::Line(a * z, b * z, c * z, d * z),
        Shape::Circle(x, y, r) => Shape::Circle(x * z, y * z, r * z),
    }
}

pub(super) fn diagram(
    d: &Designer,
    window: &Window,
    cx: &mut Context<Designer>,
) -> impl IntoElement + use<> {
    let theme = cx.theme().clone();
    let z = d.zoom;
    let geo = &d.derived.geo;
    let (width, height) = (
        (geo.size.0 + 2.0 * MARGIN) * z,
        (geo.size.1 + 2.0 * MARGIN) * z,
    );
    // 図が小さくても、画面いっぱいを操作の面にする(どこでも、ズームの操作が効くように)
    let view = window.viewport_size();
    let (min_w, min_h) = (f32::from(view.width) - 24.0, f32::from(view.height) - 100.0);

    let mut lines = Vec::new();
    let mut shapes = Vec::new();
    for e in &geo.edges {
        lines.push(
            e.points
                .iter()
                .map(|p| ((p.0 + MARGIN) * z, (p.1 + MARGIN) * z))
                .collect::<Vec<_>>(),
        );
        let ends = [
            (e.child_marker, e.points[0], e.child_dir),
            (
                e.parent_marker,
                *e.points.last().unwrap_or(&e.points[0]),
                e.parent_dir,
            ),
        ];
        for (m, (x, y), dir) in ends {
            shapes.extend(
                marker_shapes(m, x + MARGIN, y + MARGIN, dir)
                    .into_iter()
                    .map(|s| scaled(s, z)),
            );
        }
    }
    let (line_color, background) = (theme.muted_foreground, theme.background);
    let edges = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            paint_edges(bounds, &lines, &shapes, line_color, background, window)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .w(px(width))
    .h(px(height));

    let boxes: Vec<AnyElement> = (0..d.design.tables.len())
        .map(|i| table_box(d, i, cx).into_any_element())
        .collect();

    div()
        .id("designer-canvas")
        .size_full()
        .overflow_scroll()
        .bg(theme.muted.opacity(0.35))
        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
            if this.drag.is_some() {
                this.drag_to(ev.position, cx);
            } else if this.resize.is_some() {
                this.resize_to(ev.position, cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, w, cx| this.end_drag(w, cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, w, cx| this.end_drag(w, cx)),
        )
        .child(
            div()
                .relative()
                .w(px(width.max(min_w)))
                .h(px(height.max(min_h)))
                // ⌘(Ctrl)を押しながらスクロールで、図全体の拡大・縮小
                .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                    if ev.modifiers.platform || ev.modifiers.control {
                        let dy = f32::from(ev.delta.pixel_delta(px(16.)).y);
                        this.set_zoom(this.zoom * (1.0 + dy * 0.004), cx);
                        cx.stop_propagation();
                    }
                }))
                // 何もない所をクリックしたら、選択をやめる
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, w, cx| {
                        this.commit_inline(w, cx);
                        this.type_menu = None;
                        this.selected = None;
                        this.pick = None;
                        cx.notify();
                    }),
                )
                .child(edges)
                .children(edge_hits(d, cx))
                .children(boxes)
                // 大きさを変えている間は、ポインタが角から外れても、矢印をそのままにする
                .when(d.resize.is_some(), |c| {
                    c.child(canvas(
                        |_, _, _| (),
                        |_, _, window, _| {
                            window.set_window_cursor_style(CursorStyle::ResizeUpLeftDownRight)
                        },
                    ))
                })
                .children(type_popup(d, cx)),
        )
}
