//! スキーマツリーの行を作る。描画に依存しない純粋な関数(単体テストできる)。

use sqltool_core::{KeyKind, TableKind};

use crate::model::{SchemaState, schema_key, table_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Schema {
        system: bool,
    },
    Table(TableKind),
    Column {
        primary: bool,
    },
    Key,
    Index,
    /// 読み込み中、空、エラーなどの補足
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// 開閉できる行のキー。`None` なら葉
    pub key: Option<String>,
    pub depth: usize,
    pub label: String,
    /// 型や定義など、ラベルの横に薄く出す補足
    pub detail: String,
    pub kind: RowKind,
    pub expanded: bool,
    /// テーブルの行なら、(スキーマ名, 表名)。「開く」で、編集できる形で全行を開く
    pub table: Option<(String, String)>,
}

fn leaf(
    depth: usize,
    label: impl Into<String>,
    detail: impl Into<String>,
    kind: RowKind,
) -> TreeRow {
    TreeRow {
        key: None,
        depth,
        label: label.into(),
        detail: detail.into(),
        kind,
        expanded: false,
        table: None,
    }
}

fn note(depth: usize, text: &str) -> TreeRow {
    leaf(depth, text, "", RowKind::Note)
}

pub fn build(state: &SchemaState) -> Vec<TreeRow> {
    if let Some(e) = &state.error {
        return vec![note(0, e)];
    }
    let Some(schemas) = &state.schemas else {
        return vec![note(0, "読み込み中…")];
    };
    let mut rows = Vec::new();
    for s in schemas {
        let key = schema_key(&s.name);
        let expanded = state.is_expanded(&key);
        rows.push(TreeRow {
            key: Some(key),
            depth: 0,
            label: s.name.clone(),
            detail: String::new(),
            kind: RowKind::Schema { system: s.system },
            expanded,
            table: None,
        });
        if expanded {
            push_tables(&mut rows, state, &s.name);
        }
    }
    rows
}

fn push_tables(rows: &mut Vec<TreeRow>, state: &SchemaState, schema: &str) {
    let Some(tables) = state.tables.get(schema) else {
        rows.push(note(1, "読み込み中…"));
        return;
    };
    if tables.is_empty() {
        rows.push(note(1, "(テーブルなし)"));
    }
    for t in tables {
        let key = table_key(schema, &t.name);
        let expanded = state.is_expanded(&key);
        rows.push(TreeRow {
            key: Some(key),
            depth: 1,
            label: t.name.clone(),
            detail: String::new(),
            kind: RowKind::Table(t.kind),
            expanded,
            table: Some((schema.to_string(), t.name.clone())),
        });
        if expanded {
            push_details(rows, state, schema, &t.name);
        }
    }
}

fn push_details(rows: &mut Vec<TreeRow>, state: &SchemaState, schema: &str, table: &str) {
    let Some(d) = state.details.get(&(schema.to_string(), table.to_string())) else {
        rows.push(note(2, "読み込み中…"));
        return;
    };
    let primary: Vec<&str> = d
        .keys
        .iter()
        .filter(|k| k.kind == KeyKind::Primary)
        .flat_map(|k| k.columns.iter().map(String::as_str))
        .collect();
    for c in &d.columns {
        let null = if c.nullable { "" } else { "  NOT NULL" };
        rows.push(leaf(
            2,
            c.name.clone(),
            format!("{}{null}", c.type_name),
            RowKind::Column {
                primary: primary.contains(&c.name.as_str()),
            },
        ));
    }
    for k in &d.keys {
        let tag = match k.kind {
            KeyKind::Primary => "PK",
            KeyKind::Unique => "UQ",
            KeyKind::Foreign => "FK",
            KeyKind::Check => "CK",
            KeyKind::Exclusion => "EX",
        };
        let detail = match (&k.definition, k.columns.is_empty()) {
            (Some(def), true) => def.clone(),
            _ => k.columns.join(", "),
        };
        rows.push(leaf(2, format!("{tag}  {}", k.name), detail, RowKind::Key));
    }
    for i in &d.indexes {
        let unique = if i.unique { "UNIQUE " } else { "" };
        rows.push(leaf(
            2,
            format!("IX  {}", i.name),
            format!("{unique}{} ({})", i.method, i.columns.join(", ")),
            RowKind::Index,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqltool_core::{ColumnInfo, KeyInfo, SchemaInfo, TableDetails, TableInfo, TableRef};

    fn state() -> SchemaState {
        SchemaState {
            schemas: Some(vec![
                SchemaInfo {
                    name: "app".into(),
                    system: false,
                },
                SchemaInfo {
                    name: "pg_catalog".into(),
                    system: true,
                },
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn a_closed_tree_lists_only_schemas() {
        let rows = build(&state());
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            ["app", "pg_catalog"]
        );
        assert!(rows.iter().all(|r| r.key.is_some() && !r.expanded));
        assert_eq!(rows[1].kind, RowKind::Schema { system: true });
    }

    #[test]
    fn loading_and_error_are_shown_as_notes() {
        assert_eq!(build(&SchemaState::default())[0].label, "読み込み中…");
        let s = SchemaState {
            error: Some("接続できません".into()),
            ..Default::default()
        };
        assert_eq!(build(&s)[0].kind, RowKind::Note);
    }

    #[test]
    fn expanding_a_schema_shows_loading_until_tables_arrive() {
        let mut s = state();
        s.toggle(&schema_key("app"));
        assert_eq!(build(&s)[1].label, "読み込み中…");
        s.tables.insert(
            "app".into(),
            vec![TableInfo {
                schema: "app".into(),
                name: "users".into(),
                kind: TableKind::Table,
            }],
        );
        let rows = build(&s);
        assert_eq!(rows[1].label, "users");
        assert_eq!(rows[1].depth, 1);
        s.tables.insert("app".into(), vec![]);
        assert_eq!(build(&s)[1].label, "(テーブルなし)");
    }

    #[test]
    fn table_details_list_columns_keys_and_indexes_with_primary_marked() {
        let mut s = state();
        s.toggle(&schema_key("app"));
        s.toggle(&table_key("app", "users"));
        s.tables.insert(
            "app".into(),
            vec![TableInfo {
                schema: "app".into(),
                name: "users".into(),
                kind: TableKind::Table,
            }],
        );
        s.details.insert(
            ("app".into(), "users".into()),
            TableDetails {
                table: TableRef::new("app", "users"),
                columns: vec![
                    ColumnInfo {
                        name: "id".into(),
                        type_name: "bigint".into(),
                        nullable: false,
                        default: None,
                        ordinal: 1,
                    },
                    ColumnInfo {
                        name: "name".into(),
                        type_name: "text".into(),
                        nullable: true,
                        default: None,
                        ordinal: 2,
                    },
                ],
                keys: vec![KeyInfo {
                    name: "users_pkey".into(),
                    kind: KeyKind::Primary,
                    columns: vec!["id".into()],
                    definition: None,
                }],
                indexes: vec![],
            },
        );
        let rows = build(&s);
        let col = |i: usize| {
            (
                rows[i].label.clone(),
                rows[i].detail.clone(),
                rows[i].kind.clone(),
            )
        };
        assert_eq!(
            col(2),
            (
                "id".into(),
                "bigint  NOT NULL".into(),
                RowKind::Column { primary: true }
            )
        );
        assert_eq!(
            col(3),
            (
                "name".into(),
                "text".into(),
                RowKind::Column { primary: false }
            )
        );
        assert_eq!(col(4), ("PK  users_pkey".into(), "id".into(), RowKind::Key));
    }

    #[test]
    fn names_containing_dots_do_not_collide() {
        assert_ne!(table_key("a.b", "c"), table_key("a", "b.c"));
    }
}
