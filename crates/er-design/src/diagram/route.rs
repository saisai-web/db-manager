//! 関係線の経路。子の表の外部キーの列から、親の表の参照先の列へ、直角に折れる線を引く。

use super::shapes::{MARKER_LEN, Marker};
use super::{Labels, Rect, box_size, resolve_positions, row_center};
use crate::model::Design;

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// 子の表(外部キーを持つ側)の添字と、その `foreign_keys` の添字
    pub child: usize,
    pub fk: usize,
    pub parent: usize,
    /// 折れ線の頂点(子の端 → 親の端)
    pub points: Vec<(f32, f32)>,
    pub child_marker: Marker,
    pub parent_marker: Marker,
    /// 子の端・親の端で、線が箱から出ていく向き(右が 1、左が -1)
    pub child_dir: f32,
    pub parent_dir: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub rects: Vec<Rect>,
    pub edges: Vec<Edge>,
    /// 図全体の大きさ(余白を含まない)
    pub size: (f32, f32),
}

/// 箱の縁から線が真横に出る長さの下限(印のため)
const STUB: f32 = MARKER_LEN;

pub fn geometry(d: &Design, labels: Labels) -> Geometry {
    let positions = resolve_positions(d, labels);
    let rects: Vec<Rect> = d
        .tables
        .iter()
        .zip(&positions)
        .map(|(t, &(x, y))| {
            let (w, h) = box_size(t, labels);
            Rect { x, y, w, h }
        })
        .collect();
    let mut edges = Vec::new();
    for (ci, t) in d.tables.iter().enumerate() {
        for (fi, fk) in t.foreign_keys.iter().enumerate() {
            let Some(pi) = d.table_index(&fk.ref_table) else {
                continue;
            };
            let parent = &d.tables[pi];
            let ref_cols = d.ref_columns(fk);
            let row = |tbl: &crate::model::Table, col: Option<&String>| {
                let index = col
                    .and_then(|c| tbl.columns.iter().position(|x| &x.name == c))
                    .unwrap_or(0);
                row_center(index, tbl.factor())
            };
            let y1 = rects[ci].y + row(t, fk.columns.first());
            let mut y2 = rects[pi].y + row(parent, ref_cols.first());
            if ci == pi && (y1 - y2).abs() < 1.0 {
                y2 += 14.0;
            }
            let lane = edges.len() % 5;
            let (points, child_dir, parent_dir) =
                route(rects[ci], rects[pi], ci == pi, y1, y2, lane);
            edges.push(Edge {
                child: ci,
                fk: fi,
                parent: pi,
                points,
                child_marker: if t.fk_is_one_to_one(fk) {
                    Marker::ZeroOrOne
                } else {
                    Marker::Many
                },
                parent_marker: if t.fk_required(fk) {
                    Marker::One
                } else {
                    Marker::ZeroOrOne
                },
                child_dir,
                parent_dir,
            });
        }
    }
    separate_lanes(&mut edges);
    let width = rects.iter().map(Rect::right).fold(0.0, f32::max);
    let height = rects.iter().map(Rect::bottom).fold(0.0, f32::max);
    let (ex, ey) = edges
        .iter()
        .flat_map(|e| e.points.iter())
        .fold((0.0f32, 0.0f32), |m, p| (m.0.max(p.0), m.1.max(p.1)));
    Geometry {
        rects,
        edges,
        size: (width.max(ex), height.max(ey)),
    }
}

/// (頂点, 子の端の向き, 親の端の向き)
fn route(
    c: Rect,
    p: Rect,
    same: bool,
    y1: f32,
    y2: f32,
    lane: usize,
) -> (Vec<(f32, f32)>, f32, f32) {
    let shift = lane as f32 * 6.0 - 12.0;
    if same {
        let out = c.right() + STUB + 14.0;
        return (
            vec![(c.right(), y1), (out, y1), (out, y2), (p.right(), y2)],
            1.0,
            1.0,
        );
    }
    let gap = |from_right: f32, to_left: f32| (from_right + to_left) / 2.0 + shift;
    if c.x >= p.right() + 2.0 * STUB {
        // 子が右: 子の左端 → 親の右端
        let mid = gap(p.right(), c.x);
        (
            vec![(c.x, y1), (mid, y1), (mid, y2), (p.right(), y2)],
            -1.0,
            1.0,
        )
    } else if c.right() + 2.0 * STUB <= p.x {
        // 子が左: 子の右端 → 親の左端
        let mid = gap(c.right(), p.x);
        (
            vec![(c.right(), y1), (mid, y1), (mid, y2), (p.x, y2)],
            1.0,
            -1.0,
        )
    } else {
        // 横に重なる: 右側を回り込む
        let out = c.right().max(p.right()) + STUB + 14.0 + lane as f32 * 6.0;
        (
            vec![(c.right(), y1), (out, y1), (out, y2), (p.right(), y2)],
            1.0,
            1.0,
        )
    }
}

/// 縦に走る区間(`points[1]` と `points[2]` の x)が、ほかの線と重なって見えないよう、右へずらす。
fn separate_lanes(edges: &mut [Edge]) {
    const STEP: f32 = 8.0;
    let mut placed: Vec<(f32, f32, f32)> = Vec::new(); // (x, 上, 下)
    for e in edges.iter_mut().filter(|e| e.points.len() == 4) {
        let (top, bottom) = {
            let (a, b) = (e.points[1].1, e.points[2].1);
            (a.min(b), a.max(b))
        };
        let mut x = e.points[1].0;
        while placed
            .iter()
            .any(|&(px, pt, pb)| (px - x).abs() < STEP - 0.5 && pt < bottom && top < pb)
        {
            x += STEP;
        }
        e.points[1].0 = x;
        e.points[2].0 = x;
        placed.push((x, top, bottom));
    }
}
