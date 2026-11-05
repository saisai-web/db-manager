//! 関係線の端の印(IE 記法のカラスの足)。線分と円だけで表す。

/// 関係線の端の意味。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// ちょうど 1(`||`)
    One,
    /// 0 または 1(`|o`)
    ZeroOrOne,
    /// 0 以上(`o{`)
    Many,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Line(f32, f32, f32, f32),
    Circle(f32, f32, f32),
}

/// 箱の縁の点 `(x, y)` につける印。`dir` は箱から線が出ていく向き(右が 1、左が -1)。
pub fn marker_shapes(m: Marker, x: f32, y: f32, dir: f32) -> Vec<Shape> {
    let bar = |d: f32| Shape::Line(x + dir * d, y - 6.0, x + dir * d, y + 6.0);
    match m {
        Marker::One => vec![bar(7.0), bar(12.0)],
        Marker::ZeroOrOne => vec![bar(7.0), Shape::Circle(x + dir * 16.0, y, 4.0)],
        Marker::Many => vec![
            Shape::Line(x + dir * 12.0, y, x, y - 6.0),
            Shape::Line(x + dir * 12.0, y, x, y + 6.0),
            Shape::Circle(x + dir * 17.0, y, 4.0),
        ],
    }
}

/// 印の分だけ、線をまっすぐ伸ばす長さ(印と曲がり角が重ならないように)。
pub const MARKER_LEN: f32 = 22.0;
