//! テーブルの SELECT / INSERT / UPDATE / DELETE の雛形づくり。右クリックのメニューから使う。
//! 値は `NULL` にしてあり、そのまま実行しても(UPDATE / DELETE は何にも一致せず)被害が出ない。

use std::sync::Arc;

use driver_api::{Dialect, KeyKind};

use crate::core::{Res, Shared};
use crate::types::*;

/// 「先頭の行を表示」の行数(SELECT の雛形も、この行数で絞る)
pub const SELECT_PREVIEW_ROWS: u64 = 100;

impl Shared {
    pub(crate) async fn generate_sql(
        self: &Arc<Self>,
        tab_id: TabId,
        table: TableRef,
        kind: SqlKind,
    ) -> Res {
        let tab = self.tab(tab_id)?;
        let sql = match kind {
            SqlKind::Select | SqlKind::SelectAll => {
                build_select(kind, tab.driver.dialect(), &table)
            }
            _ => {
                let connection = tab.connection.id.clone();
                let cache = self.schema_cache(&connection);
                let connect = self.connector(connection);
                let details = cache.details(&connect, &table).await?;
                build(kind, tab.driver.dialect(), &table, &details)
            }
        };
        self.emit(Event::SqlGenerated {
            tab: tab_id,
            table,
            kind,
            sql,
        });
        Ok(())
    }
}

fn build_select(kind: SqlKind, d: &dyn Dialect, table: &TableRef) -> String {
    let limit = (kind == SqlKind::Select).then_some(SELECT_PREVIEW_ROWS);
    d.select_table_sql(table, limit)
}

/// 列の一覧を持つ、INSERT / UPDATE / DELETE の雛形。
fn build(kind: SqlKind, d: &dyn Dialect, table: &TableRef, details: &TableDetails) -> String {
    let name = d.qualified_name(table);
    let cols: Vec<&str> = details.columns.iter().map(|c| c.name.as_str()).collect();
    let primary: Vec<&str> = details
        .keys
        .iter()
        .find(|k| k.kind == KeyKind::Primary)
        .map(|k| k.columns.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let list = |names: &[&str], sep: &str, each: &dyn Fn(&str) -> String| {
        names.iter().map(|n| each(n)).collect::<Vec<_>>().join(sep)
    };
    let condition = if primary.is_empty() {
        // キーが無い表は、利用者に条件を決めてもらう
        "WHERE 条件 = NULL".to_string()
    } else {
        format!(
            "WHERE {}",
            list(&primary, "\n  AND ", &|c| format!(
                "{} = NULL",
                d.quote_ident(c)
            ))
        )
    };
    match kind {
        SqlKind::Select | SqlKind::SelectAll => build_select(kind, d, table),
        SqlKind::Insert => format!(
            "INSERT INTO {name} (\n  {}\n)\nVALUES (\n  {}\n);",
            list(&cols, ",\n  ", &|c| d.quote_ident(c)),
            list(&cols, ",\n  ", &|_| "NULL".to_string()),
        ),
        SqlKind::Update => {
            // 主キーの列は、条件に使うので SET には入れない(全列が主キーなら、全列を入れる)
            let set: Vec<&str> = cols
                .iter()
                .copied()
                .filter(|c| !primary.contains(c))
                .collect();
            let set = if set.is_empty() { &cols } else { &set };
            format!(
                "UPDATE {name}\nSET\n  {}\n{condition};",
                list(set, ",\n  ", &|c| format!("{} = NULL", d.quote_ident(c)))
            )
        }
        SqlKind::Delete => format!("DELETE FROM {name}\n{condition};"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::{ColumnInfo, KeyInfo, LexicalFeatures};

    struct Pg;

    impl Dialect for Pg {
        fn name(&self) -> &'static str {
            "pg"
        }
        fn quote_ident(&self, i: &str) -> String {
            format!("\"{i}\"")
        }
        fn quote_literal(&self, v: &str) -> String {
            format!("'{v}'")
        }
        fn lexical_features(&self) -> LexicalFeatures {
            LexicalFeatures::default()
        }
        fn bind_text(&self, n: usize, _: &str) -> String {
            format!("${n}")
        }
        fn select_table_sql(&self, t: &TableRef, limit: Option<u64>) -> String {
            format!(
                "SELECT * FROM {}{}",
                self.qualified_name(t),
                limit.map(|l| format!(" LIMIT {l}")).unwrap_or_default()
            )
        }
        fn set_read_only_sql(&self, _: bool) -> Option<String> {
            None
        }
        fn explain_sql(&self, _: &str, _: bool) -> Option<String> {
            None
        }
    }

    fn details(primary: &[&str]) -> TableDetails {
        let col = |n: &str, i| ColumnInfo {
            name: n.into(),
            type_name: "text".into(),
            nullable: true,
            default: None,
            ordinal: i,
        };
        TableDetails {
            table: TableRef::new("public", "t"),
            columns: vec![col("id", 1), col("name", 2)],
            keys: if primary.is_empty() {
                vec![]
            } else {
                vec![KeyInfo {
                    name: "pk".into(),
                    kind: KeyKind::Primary,
                    columns: primary.iter().map(|s| s.to_string()).collect(),
                    definition: None,
                }]
            },
            indexes: vec![],
        }
    }

    #[test]
    fn update_and_delete_are_keyed_by_the_primary_key() {
        let t = TableRef::new("public", "t");
        let d = details(&["id"]);
        assert_eq!(
            build(SqlKind::Update, &Pg, &t, &d),
            "UPDATE \"public\".\"t\"\nSET\n  \"name\" = NULL\nWHERE \"id\" = NULL;"
        );
        assert_eq!(
            build(SqlKind::Delete, &Pg, &t, &d),
            "DELETE FROM \"public\".\"t\"\nWHERE \"id\" = NULL;"
        );
    }

    #[test]
    fn insert_lists_every_column_and_a_table_without_a_key_gets_a_placeholder_condition() {
        let t = TableRef::new("public", "t");
        let d = details(&[]);
        assert_eq!(
            build(SqlKind::Insert, &Pg, &t, &d),
            "INSERT INTO \"public\".\"t\" (\n  \"id\",\n  \"name\"\n)\nVALUES (\n  NULL,\n  NULL\n);"
        );
        assert!(build(SqlKind::Delete, &Pg, &t, &d).contains("WHERE 条件 = NULL"));
    }
}
