//! 画面下部に出す、実行状況の 1 行。

use gpui_kit::SharedString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusLine {
    pub text: SharedString,
    pub level: Level,
}

impl StatusLine {
    pub fn new(level: Level, text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            level,
        }
    }

    pub fn info(text: impl Into<SharedString>) -> Self {
        Self::new(Level::Info, text)
    }

    pub fn ok(text: impl Into<SharedString>) -> Self {
        Self::new(Level::Ok, text)
    }

    pub fn warn(text: impl Into<SharedString>) -> Self {
        Self::new(Level::Warn, text)
    }

    pub fn error(text: impl Into<SharedString>) -> Self {
        Self::new(Level::Error, text)
    }
}

impl Default for StatusLine {
    fn default() -> Self {
        Self::info("準備完了")
    }
}
