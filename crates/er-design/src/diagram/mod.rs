//! 図の形。表の箱の大きさ・自動配置・関係線の経路を計算する(描画は、`svg` と画面側が行う)。
//! 座標は、図の左上を原点とする。

mod arrange;
mod route;
mod shapes;
#[cfg(test)]
mod tests;

pub use arrange::{auto_positions, resolve_positions};
pub use route::{Edge, Geometry, geometry};
pub use shapes::{Marker, Shape, marker_shapes};

use crate::model::{Column, Table};

/// 箱の中に何を書くか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Labels {
    /// 物理名と型(物理 ER 図)
    #[default]
    Physical,
    /// 論理名だけ(論理 ER 図)。論理名がなければ物理名
    Logical,
}

pub const HEADER_H: f32 = 30.0;
pub const ROW_H: f32 = 22.0;
/// 箱の下の余白
pub const FOOT_H: f32 = 6.0;
/// 表を大きくしたとき、列の行の高さが増える割合(増えた分の大半は、箱の下の余白になる)
pub const ROW_GROW: f32 = 0.15;
pub const MIN_WIDTH: f32 = 170.0;
/// 箱の左端から名前までの、主キー / 外部キーの印の幅
pub const BADGE_W: f32 = 42.0;
pub const PAD: f32 = 10.0;
/// 列名の欄の幅の下限(その場で名前を打てる広さ)
pub const NAME_MIN_W: f32 = 80.0;
pub const GAP_X: f32 = 110.0;
pub const GAP_Y: f32 = 36.0;
/// 見出しの右端に置く「＋」「詳細」ボタンの幅(画面だけが使うが、箱の幅に含める)
pub const HEADER_BTN_W: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// 見出しの名前(13px の太字)の幅の、`text_width` に対する倍率。
pub const HEADER_SCALE: f32 = 1.25;

/// 12px の文字列の幅の見積もり(半角は 6.6、全角は 12)。
pub fn text_width(s: &str) -> f32 {
    s.chars()
        .map(|c| if c.is_ascii() { 6.6 } else { 12.0 })
        .sum()
}

/// 列の行に書く名前と型。
pub fn column_label(c: &Column, labels: Labels) -> (String, String) {
    match labels {
        Labels::Physical => (c.name.clone(), c.ty.to_string()),
        Labels::Logical => (
            if c.logical.is_empty() {
                c.name.clone()
            } else {
                c.logical.clone()
            },
            String::new(),
        ),
    }
}

/// 箱の見出しに書く文字列(主, 副)。副は薄く小さく添える。
pub fn header_label(t: &Table, labels: Labels) -> (String, String) {
    match labels {
        Labels::Physical => (t.name.clone(), t.logical.clone()),
        Labels::Logical => (t.display_name().to_string(), String::new()),
    }
}

pub fn box_size(t: &Table, labels: Labels) -> (f32, f32) {
    let (main, sub) = header_label(t, labels);
    let sub_w = if sub.is_empty() {
        0.0
    } else {
        text_width(&sub) * 0.9 + 10.0
    };
    let mut w = text_width(&main) * HEADER_SCALE + sub_w + 2.0 * PAD + HEADER_BTN_W;
    for c in &t.columns {
        let (name, ty) = column_label(c, labels);
        let ty_w = if ty.is_empty() {
            0.0
        } else {
            text_width(&ty) + 16.0
        };
        w = w.max(BADGE_W + text_width(&name).max(NAME_MIN_W) + ty_w + PAD);
    }
    let k = t.factor();
    (
        w.max(MIN_WIDTH).ceil() * k,
        // 見出しは大きくしない。列の行も少しだけ。増えた分の大半は、行の下の余白になる
        HEADER_H + ROW_H * k * t.columns.len().max(1) as f32 + FOOT_H,
    )
}

/// 列の行 1 つの高さ。表の大きさ `k` に対して、ほんの少ししか増えない。
pub fn row_height(k: f32) -> f32 {
    ROW_H * (1.0 + (k - 1.0) * ROW_GROW)
}

/// 箱の上端から、`index` 番目の列の行の中央までの距離。
pub fn row_center(index: usize, k: f32) -> f32 {
    HEADER_H + row_height(k) * (index as f32 + 0.5)
}
