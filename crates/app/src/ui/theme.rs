//! 色と書体。意味のある色(本番、エラー、成功)をここに集める。

use super::prelude::*;

/// 本番ラベルの赤。画面全体の枠とバッジに使う。
pub fn production_red() -> Hsla {
    hsla(0.0, 0.78, 0.50, 1.0)
}

pub fn ok_green() -> Hsla {
    hsla(0.37, 0.60, 0.36, 1.0)
}

pub fn warn_amber() -> Hsla {
    hsla(0.10, 0.90, 0.42, 1.0)
}

pub fn error_red() -> Hsla {
    hsla(0.0, 0.70, 0.45, 1.0)
}

/// SQL の既定の等幅フォント(設計書: Menlo + Hiragino Sans / Consolas + Meiryo)。
pub fn mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "Consolas"
    }
}

/// 編集して、まだ保存していないセルの背景。
pub fn edited_background() -> Hsla {
    hsla(0.12, 0.90, 0.60, 0.25)
}
