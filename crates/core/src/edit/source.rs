//! 編集できるテーブルかの判定と、編集に必要なテーブルの情報。

use driver_api::{ColumnInfo, KeyKind, TableKind, TableRef};

use crate::types::TableDetails;

/// グリッドで編集できるテーブル。結果の列は、`columns` と名前で対応する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSource {
    pub table: TableRef,
    pub columns: Vec<ColumnInfo>,
    /// 行を特定するキーの列名(主キー、または NOT NULL の一意キー)
    pub key: Vec<String>,
}

/// 結果が、グリッドで編集できるか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditAccess {
    /// スキーマツリーから開いたテーブルではない(任意の SQL の結果)
    NotATable,
    ReadOnly(String),
    Editable(std::sync::Arc<TableSource>),
}

impl TableSource {
    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// 編集できるのは、通常のテーブルで、行を一意に特定できるものだけ。
    /// 主キーも一意キーもなければ、更新先を正確に特定できないため、読み取り専用にする。
    pub fn access(details: &TableDetails, kind: TableKind) -> EditAccess {
        if kind != TableKind::Table {
            return EditAccess::ReadOnly("ビューや外部表は編集できません".into());
        }
        let not_null = |name: &String| {
            details
                .columns
                .iter()
                .find(|c| &c.name == name)
                .is_some_and(|c| !c.nullable)
        };
        let key = details
            .keys
            .iter()
            .find(|k| k.kind == KeyKind::Primary)
            .or_else(|| {
                details
                    .keys
                    .iter()
                    .find(|k| k.kind == KeyKind::Unique && k.columns.iter().all(not_null))
            });
        match key {
            Some(k) if !k.columns.is_empty() => EditAccess::Editable(std::sync::Arc::new(Self {
                table: details.table.clone(),
                columns: details.columns.clone(),
                key: k.columns.clone(),
            })),
            _ => EditAccess::ReadOnly(
                "主キーまたは NOT NULL の一意キーがないため、読み取り専用です".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::KeyInfo;

    fn col(name: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_name: "text".into(),
            nullable,
            default: None,
            ordinal: 1,
        }
    }

    fn key(kind: KeyKind, cols: &[&str]) -> KeyInfo {
        KeyInfo {
            name: "k".into(),
            kind,
            columns: cols.iter().map(|s| s.to_string()).collect(),
            definition: None,
        }
    }

    fn details(keys: Vec<KeyInfo>) -> TableDetails {
        TableDetails {
            table: TableRef::new("public", "t"),
            columns: vec![col("id", false), col("email", true), col("code", false)],
            keys,
            indexes: vec![],
        }
    }

    fn editable_key(a: EditAccess) -> Vec<String> {
        match a {
            EditAccess::Editable(s) => s.key.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_primary_key_makes_the_table_editable() {
        let d = details(vec![key(KeyKind::Primary, &["id"])]);
        assert_eq!(
            editable_key(TableSource::access(&d, TableKind::Table)),
            ["id"]
        );
    }

    #[test]
    fn a_not_null_unique_key_is_accepted_but_a_nullable_one_is_not() {
        let d = details(vec![key(KeyKind::Unique, &["code"])]);
        assert_eq!(
            editable_key(TableSource::access(&d, TableKind::Table)),
            ["code"]
        );
        // NULL を含みうる一意キーでは、行を特定できない
        let d = details(vec![key(KeyKind::Unique, &["email"])]);
        assert!(matches!(
            TableSource::access(&d, TableKind::Table),
            EditAccess::ReadOnly(_)
        ));
    }

    #[test]
    fn the_primary_key_is_preferred_over_unique_keys() {
        let d = details(vec![
            key(KeyKind::Unique, &["code"]),
            key(KeyKind::Primary, &["id"]),
        ]);
        assert_eq!(
            editable_key(TableSource::access(&d, TableKind::Table)),
            ["id"]
        );
    }

    #[test]
    fn tables_without_a_key_and_non_tables_are_read_only() {
        let d = details(vec![key(KeyKind::Check, &["id"])]);
        assert!(matches!(
            TableSource::access(&d, TableKind::Table),
            EditAccess::ReadOnly(_)
        ));
        let d = details(vec![key(KeyKind::Primary, &["id"])]);
        for kind in [
            TableKind::View,
            TableKind::MaterializedView,
            TableKind::ForeignTable,
        ] {
            assert!(matches!(
                TableSource::access(&d, kind),
                EditAccess::ReadOnly(_)
            ));
        }
    }
}
