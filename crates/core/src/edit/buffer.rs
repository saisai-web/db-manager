//! 画面上の変更の保持。編集内容は DB に送らず、ここに溜めておく。

use std::collections::{BTreeMap, BTreeSet};

use super::change::{Change, Value};
use super::source::TableSource;

/// 編集中のセル、削除する行、追加する行。行は、結果ストア上の行番号(並べ替え前)で指す。
#[derive(Debug, Default, Clone)]
pub struct EditBuffer {
    updates: BTreeMap<u64, BTreeMap<usize, Value>>,
    deleted: BTreeSet<u64>,
    /// 追加する行。外側の `None` は「指定なし(DB の既定値)」、内側の `None` は NULL
    inserts: Vec<Vec<Option<Value>>>,
}

impl EditBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_dirty(&self) -> bool {
        !(self.updates.is_empty() && self.deleted.is_empty() && self.inserts.is_empty())
    }

    /// (更新する行数, 削除する行数, 追加する行数)
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.updates.len(), self.deleted.len(), self.inserts.len())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    // ---- 既存の行 ----

    /// セルを編集する。元の値に戻したら、変更とはみなさない。
    pub fn set_cell(&mut self, row: u64, col: usize, value: Value, original: &Value) {
        if &value == original {
            if let Some(cols) = self.updates.get_mut(&row) {
                cols.remove(&col);
                if cols.is_empty() {
                    self.updates.remove(&row);
                }
            }
        } else {
            self.updates.entry(row).or_default().insert(col, value);
        }
    }

    /// 編集した値。編集していなければ `None`。
    pub fn edited(&self, row: u64, col: usize) -> Option<&Value> {
        self.updates.get(&row)?.get(&col)
    }

    pub fn delete_row(&mut self, row: u64) {
        self.deleted.insert(row);
    }

    pub fn restore_row(&mut self, row: u64) {
        self.deleted.remove(&row);
    }

    pub fn is_deleted(&self, row: u64) -> bool {
        self.deleted.contains(&row)
    }

    // ---- 追加する行 ----

    /// 空の行を追加して、その番号(0 始まり)を返す。
    pub fn add_insert(&mut self, columns: usize) -> usize {
        self.inserts.push(vec![None; columns]);
        self.inserts.len() - 1
    }

    pub fn set_insert_cell(&mut self, ix: usize, col: usize, value: Option<Value>) {
        if let Some(cell) = self.inserts.get_mut(ix).and_then(|r| r.get_mut(col)) {
            *cell = value;
        }
    }

    pub fn insert_cell(&self, ix: usize, col: usize) -> Option<&Option<Value>> {
        self.inserts.get(ix)?.get(col)
    }

    pub fn remove_insert(&mut self, ix: usize) {
        if ix < self.inserts.len() {
            self.inserts.remove(ix);
        }
    }

    // ---- 保存する変更 ----

    /// 保存する変更にする。`columns` は結果の列名(結果の列順)、`original(行, 列)` は元の値。
    /// 順序は、削除、更新、追加(削除で空いた一意な値を、追加が使えるように)。
    pub fn to_changes(
        &self,
        source: &TableSource,
        columns: &[String],
        original: impl Fn(u64, usize) -> Value,
    ) -> Vec<Change> {
        let key_of = |row: u64| -> Vec<(String, Value)> {
            source
                .key
                .iter()
                .filter_map(|k| {
                    let ix = columns.iter().position(|c| c == k)?;
                    Some((k.clone(), original(row, ix)))
                })
                .collect()
        };
        let mut out = Vec::new();
        for &row in &self.deleted {
            out.push(Change::Delete { key: key_of(row) });
        }
        for (&row, cols) in &self.updates {
            if self.deleted.contains(&row) {
                continue; // 削除する行の編集は、捨てる
            }
            let set: Vec<_> = cols
                .iter()
                .map(|(&c, new)| (columns[c].clone(), new.clone(), original(row, c)))
                .collect();
            out.push(Change::Update {
                key: key_of(row),
                set,
            });
        }
        for row in &self.inserts {
            let values: Vec<_> = row
                .iter()
                .enumerate()
                .filter_map(|(c, v)| v.clone().map(|v| (columns[c].clone(), v)))
                .collect();
            out.push(Change::Insert { values });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::{ColumnInfo, TableRef};

    fn source() -> TableSource {
        let col = |n: &str| ColumnInfo {
            name: n.into(),
            type_name: "text".into(),
            nullable: true,
            default: None,
            ordinal: 1,
        };
        TableSource {
            table: TableRef::new("public", "t"),
            columns: vec![col("id"), col("name"), col("note")],
            key: vec!["id".into()],
        }
    }

    fn cols() -> Vec<String> {
        ["id", "name", "note"].map(String::from).to_vec()
    }

    fn s(v: &str) -> Value {
        Some(v.to_string())
    }

    /// 元のデータ: 行 r の列 c は "r{r}c{c}"。ただし行 3 の note は NULL
    fn original(r: u64, c: usize) -> Value {
        if r == 3 && c == 2 {
            None
        } else {
            s(&format!("r{r}c{c}"))
        }
    }

    #[test]
    fn a_new_buffer_is_clean() {
        let b = EditBuffer::new();
        assert!(!b.is_dirty());
        assert_eq!(b.counts(), (0, 0, 0));
        assert!(b.to_changes(&source(), &cols(), original).is_empty());
    }

    #[test]
    fn editing_a_cell_back_to_its_original_value_is_not_a_change() {
        let mut b = EditBuffer::new();
        b.set_cell(1, 1, s("new"), &original(1, 1));
        assert!(b.is_dirty());
        assert_eq!(b.edited(1, 1), Some(&s("new")));
        b.set_cell(1, 1, s("r1c1"), &original(1, 1));
        assert!(!b.is_dirty(), "reverting must clean the row up");
        assert_eq!(b.edited(1, 1), None);
    }

    #[test]
    fn null_and_empty_string_are_different_values() {
        let mut b = EditBuffer::new();
        // 元が NULL のセルを空文字列にするのは、変更
        b.set_cell(3, 2, s(""), &original(3, 2));
        assert!(b.is_dirty());
        // 元が値のセルを NULL にするのも、変更
        b.set_cell(1, 2, None, &original(1, 2));
        let changes = b.to_changes(&source(), &cols(), original);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|c| matches!(c,
            Change::Update { set, .. } if set == &vec![("note".to_string(), s(""), None)])));
        assert!(changes.iter().any(|c| matches!(c,
            Change::Update { set, .. } if set == &vec![("note".to_string(), None, s("r1c2"))])));
    }

    #[test]
    fn update_carries_the_key_and_the_original_values() {
        let mut b = EditBuffer::new();
        b.set_cell(2, 1, s("A"), &original(2, 1));
        b.set_cell(2, 2, s("B"), &original(2, 2));
        let changes = b.to_changes(&source(), &cols(), original);
        let [Change::Update { key, set }] = changes.as_slice() else {
            panic!()
        };
        assert_eq!(key, &vec![("id".to_string(), s("r2c0"))]);
        assert_eq!(
            set,
            &vec![
                ("name".to_string(), s("A"), s("r2c1")),
                ("note".to_string(), s("B"), s("r2c2")),
            ]
        );
    }

    #[test]
    fn edits_to_a_deleted_row_are_dropped_and_restoring_brings_them_back() {
        let mut b = EditBuffer::new();
        b.set_cell(4, 1, s("x"), &original(4, 1));
        b.delete_row(4);
        let changes = b.to_changes(&source(), &cols(), original);
        assert_eq!(
            changes,
            vec![Change::Delete {
                key: vec![("id".into(), s("r4c0"))]
            }]
        );
        b.restore_row(4);
        assert!(!b.is_deleted(4));
        assert!(matches!(
            b.to_changes(&source(), &cols(), original).as_slice(),
            [Change::Update { .. }]
        ));
    }

    #[test]
    fn inserts_distinguish_unset_from_null() {
        let mut b = EditBuffer::new();
        let ix = b.add_insert(3);
        b.set_insert_cell(ix, 0, Some(s("100")));
        b.set_insert_cell(ix, 2, Some(None)); // 明示的な NULL
        assert_eq!(b.insert_cell(ix, 1), Some(&None), "unset");
        let changes = b.to_changes(&source(), &cols(), original);
        let [Change::Insert { values }] = changes.as_slice() else {
            panic!()
        };
        assert_eq!(
            values,
            &vec![("id".to_string(), s("100")), ("note".to_string(), None)]
        );
        b.remove_insert(ix);
        assert!(!b.is_dirty());
    }

    #[test]
    fn changes_are_ordered_delete_update_insert() {
        let mut b = EditBuffer::new();
        let ix = b.add_insert(3);
        b.set_insert_cell(ix, 0, Some(s("9")));
        b.set_cell(1, 1, s("u"), &original(1, 1));
        b.delete_row(2);
        let kinds: Vec<_> = b
            .to_changes(&source(), &cols(), original)
            .iter()
            .map(Change::kind)
            .collect();
        use super::super::change::ChangeKind::*;
        assert_eq!(kinds, [Delete, Update, Insert]);
    }

    #[test]
    fn clear_discards_everything() {
        let mut b = EditBuffer::new();
        b.delete_row(1);
        b.add_insert(2);
        b.set_cell(1, 0, s("x"), &s("y"));
        b.clear();
        assert!(!b.is_dirty());
    }
}
