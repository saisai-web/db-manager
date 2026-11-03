//! グリッドのセルに出す内容。結果の値に、編集中の変更(追加、変更、削除)を重ねて決める。

use sqltool_core::{Access, EditBuffer, ResultHandle, ValueClass};

use super::edit_view::{RowRef, row_ref};

pub enum CellText {
    Text(String),
    /// NULL(空文字列とは別)
    Null,
    Empty,
    /// 追加する行で、値を指定していない列(DB の既定値になる)
    Default,
    /// 退避した行の読み戻し中
    Loading,
    Missing,
}

pub struct Cell {
    pub text: CellText,
    /// 編集して、元の値から変わっている(追加する行を含む)
    pub edited: bool,
    /// 削除する行
    pub deleted: bool,
}

/// バイナリは、16 進数の先頭だけ表示する。
pub fn hex_preview(bytes: &[u8]) -> String {
    const SHOWN: usize = 12;
    let head: String = bytes
        .iter()
        .take(SHOWN)
        .map(|b| format!("{b:02x}"))
        .collect();
    let more = if bytes.len() > SHOWN { "…" } else { "" };
    format!("0x{head}{more} ({} B)", bytes.len())
}

fn from_value(v: &Option<String>) -> CellText {
    match v {
        None => CellText::Null,
        Some(s) if s.is_empty() => CellText::Empty,
        Some(s) => CellText::Text(s.clone()),
    }
}

/// 受信済みの行 + 追加する行の数。
pub fn total_rows(handle: &ResultHandle, edit: Option<&EditBuffer>) -> usize {
    handle.row_count() as usize + edit.map_or(0, |e| e.counts().2)
}

pub fn resolve(
    handle: &ResultHandle,
    edit: Option<&EditBuffer>,
    position: usize,
    col: usize,
) -> Cell {
    let plain = |text| Cell {
        text,
        edited: false,
        deleted: false,
    };
    let Some(row) = row_ref(position, handle.row_count(), |p| handle.source_row(p)) else {
        return plain(CellText::Missing);
    };
    match row {
        RowRef::Inserted(ix) => {
            let text = match edit.and_then(|e| e.insert_cell(ix, col)) {
                Some(Some(v)) => from_value(v),
                _ => CellText::Default,
            };
            Cell {
                text,
                edited: true,
                deleted: false,
            }
        }
        RowRef::Stored(source) => {
            let deleted = edit.is_some_and(|e| e.is_deleted(source));
            if let Some(v) = edit.and_then(|e| e.edited(source, col)) {
                return Cell {
                    text: from_value(v),
                    edited: true,
                    deleted,
                };
            }
            let class = handle.columns()[col].class;
            let access = handle.with_source_row(source, |r| match r.cell(col) {
                None => CellText::Null,
                Some([]) => CellText::Empty,
                Some(b) if class == ValueClass::Binary => CellText::Text(hex_preview(b)),
                Some(b) => CellText::Text(String::from_utf8_lossy(b).into_owned()),
            });
            Cell {
                text: match access {
                    Access::Ready(t) => t,
                    Access::Loading => CellText::Loading,
                    Access::Unavailable => CellText::Missing,
                },
                edited: false,
                deleted,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_preview_truncates_long_values() {
        assert_eq!(hex_preview(&[0xde, 0xad]), "0xdead (2 B)");
        let long = vec![0xab; 20];
        assert_eq!(hex_preview(&long), format!("0x{}… (20 B)", "ab".repeat(12)));
    }
}
