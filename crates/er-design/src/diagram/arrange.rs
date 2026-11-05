//! 表の自動配置。参照される表(親)を左、参照する表(子)を右に並べ、線の交差が減るよう縦の順を整える。

use std::collections::HashSet;

use super::{GAP_X, GAP_Y, Labels, box_size};
use crate::model::Design;

/// (子, 親) の添字の組。自分自身への参照と、設計にない表への参照は除く。重複もまとめる。
fn edges(d: &Design) -> Vec<(usize, usize)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, t) in d.tables.iter().enumerate() {
        for f in &t.foreign_keys {
            if let Some(p) = d.table_index(&f.ref_table)
                && p != i
                && seen.insert((i, p))
            {
                out.push((i, p));
            }
        }
    }
    out
}

/// 各表の層(親の層の最大値 + 1)。循環があっても必ず終わる。
fn layers(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for &(c, p) in edges {
            if layer[c] <= layer[p] && layer[p] + 1 < n {
                layer[c] = layer[p] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    layer
}

/// 層の中の並び順を、つながっている相手の平均位置に近づけて、線の交差を減らす。
fn order_layers(columns: &mut [Vec<usize>], edges: &[(usize, usize)]) {
    // 表の番号で引くので、つながりのない表が間にあっても足りる大きさにする
    let n = columns.iter().flatten().max().map_or(0, |&m| m + 1);
    let mut pos = vec![0.0f32; n];
    let reindex = |columns: &[Vec<usize>], pos: &mut Vec<f32>| {
        for col in columns {
            for (r, &t) in col.iter().enumerate() {
                pos[t] = r as f32;
            }
        }
    };
    let neighbors = |t: usize, other_layer: &HashSet<usize>| -> Vec<usize> {
        edges
            .iter()
            .filter_map(|&(c, p)| match (c == t, p == t) {
                (true, _) => Some(p),
                (_, true) => Some(c),
                _ => None,
            })
            .filter(|o| other_layer.contains(o))
            .collect()
    };
    let size = columns.len();
    reindex(columns, &mut pos);
    for sweep in 0..6 {
        let order: Vec<usize> = if sweep % 2 == 0 {
            (1..size).collect()
        } else {
            (0..size.saturating_sub(1)).rev().collect()
        };
        for li in order {
            let from = if sweep % 2 == 0 { li - 1 } else { li + 1 };
            let other: HashSet<usize> = columns[from].iter().copied().collect();
            let mut keyed: Vec<(f32, usize, usize)> = columns[li]
                .iter()
                .enumerate()
                .map(|(r, &t)| {
                    let ns = neighbors(t, &other);
                    let key = if ns.is_empty() {
                        r as f32
                    } else {
                        ns.iter().map(|&o| pos[o]).sum::<f32>() / ns.len() as f32
                    };
                    (key, r, t)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            columns[li] = keyed.into_iter().map(|k| k.2).collect();
            reindex(columns, &mut pos);
        }
    }
}

/// すべての表の自動配置(左上の座標)。つながりのない表は、下に格子状に並べる。
pub fn auto_positions(d: &Design, labels: Labels) -> Vec<(f32, f32)> {
    let n = d.tables.len();
    let sizes: Vec<(f32, f32)> = d.tables.iter().map(|t| box_size(t, labels)).collect();
    let edges = edges(d);
    let connected: HashSet<usize> = edges.iter().flat_map(|&(c, p)| [c, p]).collect();
    let layer = layers(n, &edges);
    let depth = (0..n)
        .filter(|i| connected.contains(i))
        .map(|i| layer[i] + 1)
        .max()
        .unwrap_or(0);
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for i in (0..n).filter(|i| connected.contains(i)) {
        columns[layer[i]].push(i);
    }
    order_layers(&mut columns, &edges);

    let heights: Vec<f32> = columns
        .iter()
        .map(|c| c.iter().map(|&t| sizes[t].1 + GAP_Y).sum::<f32>() - GAP_Y)
        .collect();
    let tallest = heights.iter().copied().fold(0.0, f32::max);
    let mut out = vec![(0.0, 0.0); n];
    let mut x = 0.0;
    for (li, col) in columns.iter().enumerate() {
        let mut y = (tallest - heights[li]) / 2.0;
        for &t in col {
            out[t] = (x, y);
            y += sizes[t].1 + GAP_Y;
        }
        x += col.iter().map(|&t| sizes[t].0).fold(0.0, f32::max) + GAP_X;
    }

    // つながりのない表: 幅の合計が、つながった部分の幅くらいまで横に並べ、あふれたら次の段へ
    let loose: Vec<usize> = (0..n).filter(|i| !connected.contains(i)).collect();
    let row_limit = (x - GAP_X).max(900.0);
    let (mut lx, mut ly, mut row_h) = (
        0.0,
        if depth > 0 {
            tallest + GAP_Y * 2.0
        } else {
            0.0
        },
        0.0f32,
    );
    for t in loose {
        if lx > 0.0 && lx + sizes[t].0 > row_limit {
            lx = 0.0;
            ly += row_h + GAP_Y;
            row_h = 0.0;
        }
        out[t] = (lx, ly);
        lx += sizes[t].0 + GAP_Y;
        row_h = row_h.max(sizes[t].1);
    }
    out
}

/// 実際に使う位置: 位置が決まっている表はそのまま、決まっていない表は自動配置の位置
/// (決まっている表と重ならないよう、その下から始める)。
pub fn resolve_positions(d: &Design, labels: Labels) -> Vec<(f32, f32)> {
    if d.tables.iter().all(|t| t.pos.is_some()) {
        return d.tables.iter().filter_map(|t| t.pos).collect(); // 表の移動中に毎回呼ばれる
    }
    let auto = auto_positions(d, labels);
    let fixed_bottom = d
        .tables
        .iter()
        .filter_map(|t| t.pos.map(|p| p.1 + box_size(t, labels).1))
        .fold(None, |m: Option<f32>, b| Some(m.map_or(b, |m| m.max(b))));
    let all_free = d.tables.iter().all(|t| t.pos.is_none());
    let free_top = d
        .tables
        .iter()
        .zip(&auto)
        .filter(|(t, _)| t.pos.is_none())
        .map(|(_, p)| p.1)
        .fold(f32::MAX, f32::min);
    d.tables
        .iter()
        .zip(auto)
        .map(|(t, a)| match (t.pos, fixed_bottom) {
            (Some(p), _) => p,
            (None, Some(bottom)) if !all_free => (a.0, a.1 - free_top + bottom + GAP_Y * 2.0),
            (None, _) => a,
        })
        .collect()
}
