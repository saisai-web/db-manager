//! 画面のアイコン。部品の既定のアイコンに、ER 設計の道具で使う数点だけを足す
//! (使うものだけを埋め込むので、増えるのは数 KB)。

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(
    Extra,
    [
        FilePlus, Save, SaveAll, Table2, Trash, Link, LayoutGrid, ZoomIn, ZoomOut
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = Extra.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(Extra.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
