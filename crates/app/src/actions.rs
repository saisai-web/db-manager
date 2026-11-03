//! キー操作に対応するアクションと、そのキー割り当て。
//! 修飾キーは、macOS では Cmd、Windows では Ctrl(設計書の OS 差分)。

use gpui_kit::{App, KeyBinding, actions};

actions!(sqltool, [RunAtCursor, RunAll, CancelRun, OpenConnections]);

/// アクションを受け取る範囲を示すキーコンテキスト。
pub const CONTEXT: &str = "Workspace";

/// 修飾キーつきのキー表記(`'static` にするため、マクロで文字列を連結する)。
macro_rules! chord {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

/// SQL エディタ自身のキーコンテキスト。エディタは `Cmd/Ctrl + Enter` を改行として束縛しているため、
/// 実行のキーは同じ深さのコンテキストで、後から登録して上書きする(同じ深さでは後勝ち)。
const EDITOR_CONTEXT: &str = "Input && mode == editor";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(chord!("enter"), RunAtCursor, Some(EDITOR_CONTEXT)),
        KeyBinding::new(chord!("shift-enter"), RunAll, Some(EDITOR_CONTEXT)),
        KeyBinding::new(chord!("."), CancelRun, Some(CONTEXT)),
        KeyBinding::new(chord!("o"), OpenConnections, Some(CONTEXT)),
    ]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_binding_is_a_valid_keystroke() {
        // 不正な表記だと、起動時に panic する
        for k in [
            chord!("enter"),
            chord!("shift-enter"),
            chord!("."),
            chord!("o"),
        ] {
            gpui_kit::Keystroke::parse(k).unwrap_or_else(|e| panic!("{k}: {e:?}"));
        }
    }
}
