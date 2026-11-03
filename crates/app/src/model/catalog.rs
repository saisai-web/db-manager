//! 補完のためのスキーマ情報。モデルが持つキャッシュから作る(DB には問い合わせない)。

use sqltool_core::{Catalog, CatalogTable, ConnectionId};

use super::{AppModel, SchemaState};

/// 先読みする表の数の上限(巨大なスキーマで、問い合わせを溜めないため)
pub const PREFETCH_TABLES: usize = 200;

/// キャッシュ済みのテーブルと列。列は、読み込み済みの表だけ。
pub fn catalog(state: &SchemaState) -> Catalog {
    let mut tables = Vec::new();
    for (schema, list) in &state.tables {
        for t in list {
            let columns = state
                .details
                .get(&(schema.clone(), t.name.clone()))
                .map(|d| {
                    d.columns
                        .iter()
                        .map(|c| (c.name.clone(), c.type_name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            tables.push(CatalogTable {
                schema: schema.clone(),
                name: t.name.clone(),
                columns,
            });
        }
    }
    // 毎回同じ順序で出るよう、整えておく
    tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    Catalog { tables }
}

impl AppModel {
    pub fn catalog_for(&self, connection: &ConnectionId) -> Catalog {
        self.schemas
            .get(connection)
            .map(catalog)
            .unwrap_or_default()
    }

    /// スキーマの一覧が届いた。補完のために、システム以外のスキーマの表を先読みする。
    pub(super) fn prefetch_schemas(
        &mut self,
        connection: &ConnectionId,
        schemas: &[sqltool_core::SchemaInfo],
    ) {
        for s in schemas.iter().filter(|s| !s.system) {
            self.core.send(sqltool_core::Command::LoadTables {
                connection_id: connection.clone(),
                schema: s.name.clone(),
            });
        }
    }

    /// 表の一覧が届いた。列の情報を、まだ読み込んでいない表から先読みする。
    pub(super) fn prefetch_columns(&mut self, connection: &ConnectionId, schema: &str) {
        let Some(state) = self.schemas.get(connection) else {
            return;
        };
        let Some(tables) = state.tables.get(schema) else {
            return;
        };
        let missing: Vec<_> = tables
            .iter()
            .filter(|t| {
                !state
                    .details
                    .contains_key(&(schema.to_string(), t.name.clone()))
            })
            .take(PREFETCH_TABLES)
            .map(|t| t.name.clone())
            .collect();
        for name in missing {
            self.core.send(sqltool_core::Command::LoadTableDetails {
                connection_id: connection.clone(),
                table: sqltool_core::TableRef::new(schema, name),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqltool_core::{ColumnInfo, TableDetails, TableInfo, TableKind, TableRef};

    #[test]
    fn columns_appear_only_for_tables_whose_details_are_loaded() {
        let mut s = SchemaState::default();
        let info = |n: &str| TableInfo {
            schema: "public".into(),
            name: n.into(),
            kind: TableKind::Table,
        };
        s.tables.insert("public".into(), vec![info("b"), info("a")]);
        s.details.insert(
            ("public".into(), "a".into()),
            TableDetails {
                table: TableRef::new("public", "a"),
                columns: vec![ColumnInfo {
                    name: "id".into(),
                    type_name: "bigint".into(),
                    nullable: false,
                    default: None,
                    ordinal: 1,
                }],
                keys: vec![],
                indexes: vec![],
            },
        );
        let c = catalog(&s);
        assert_eq!(
            c.tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"],
            "sorted"
        );
        assert_eq!(
            c.tables[0].columns,
            [("id".to_string(), "bigint".to_string())]
        );
        assert!(c.tables[1].columns.is_empty());
    }
}
