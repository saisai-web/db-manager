//! 設計書の書き出し(HTML と Excel)。どちらも設計から作り、ファイルの入出力はしない。

mod css;
mod html;
mod rows;
mod xlsx;

use crate::ddl::Target;
use crate::diagram::Labels;

pub use html::html;
pub use rows::today;
pub use xlsx::xlsx;

#[derive(Debug, Clone, Default)]
pub struct DocOptions {
    /// 作成日(空なら書かない)
    pub date: String,
    /// 付録に CREATE 文を載せる DB(`None` なら載せない)
    pub ddl: Option<Target>,
    /// 図の箱に書く名前
    pub labels: Labels,
}

pub(crate) fn doc_title(d: &crate::Design) -> &str {
    if d.title.is_empty() {
        "データベース設計書"
    } else {
        &d.title
    }
}
