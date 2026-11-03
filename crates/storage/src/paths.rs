use std::path::PathBuf;

const APP_DIR: &str = "SQLTool";

/// OS ごとの既定のデータ保存先。決められなければ `None`。
///
/// macOS は `~/Library/Application Support/SQLTool`、Windows は `%APPDATA%\SQLTool`。
/// Linux は対象外だが、core 以下をビルド・テストできるよう XDG に従う。
pub fn default_data_dir() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support").join(APP_DIR))
    } else if cfg!(windows) {
        env("APPDATA").map(|a| a.join(APP_DIR))
    } else {
        env("XDG_DATA_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".local/share")))
            .map(|d| d.join(APP_DIR.to_lowercase()))
    }
}
