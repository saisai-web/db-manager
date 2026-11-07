//! 図の上での削除: 選んだ表の見出しの「×」と、列の行の「×」。

use sqltool_core::design::columns;

use super::Designer;
use crate::ui::prelude::*;

impl Designer {
    /// 表を消す。その表を参照する関係も消える。
    pub(super) fn delete_table(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        if self.selected.as_deref() == Some(name.as_str()) {
            self.selected = None;
        }
        self.edit(window, cx, |d| {
            d.remove_table(&name);
            Ok(format!(
                "表 {name} を削除しました(この表を参照する関係も消えました)"
            ))
        });
    }

    /// 列を消す。その列を使う関係・索引も消える。
    pub(super) fn delete_column(
        &mut self,
        table: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(window, cx, |d| {
            let name = d
                .tables
                .get(table)
                .and_then(|t| t.columns.get(column))
                .map(|c| c.name.clone())
                .ok_or("列が見つかりません")?;
            columns::remove_column(d, table, &name)?;
            Ok(format!(
                "列 {name} を削除しました(この列を使う関係・索引も消えました)"
            ))
        });
    }
}
