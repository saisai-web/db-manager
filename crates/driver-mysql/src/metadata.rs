use async_trait::async_trait;
use driver_api::{
    ColumnInfo, ConnectionConfig, IndexInfo, KeyInfo, KeyKind, MetadataProvider, Result,
    SchemaInfo, TableInfo, TableKind, TableRef,
};
use mysql_async::Conn;
use mysql_async::prelude::Queryable;
use tokio::sync::Mutex;

use crate::error::map_error;
use crate::open;

/// 管理用のデータベース。ツリーでは畳んで表示する。
const SYSTEM_SCHEMAS: [&str; 4] = ["mysql", "information_schema", "performance_schema", "sys"];

pub(crate) struct MysqlMetadata {
    conn: Mutex<Conn>,
}

impl MysqlMetadata {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        Ok(Self {
            conn: Mutex::new(open(cfg).await?),
        })
    }
}

/// `information_schema` の列名は、サーバーの版で大文字小文字が変わるため、位置で受け取る。
fn quote(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

#[async_trait]
impl MetadataProvider for MysqlMetadata {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>> {
        let names: Vec<String> = self
            .conn
            .lock()
            .await
            .query("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME")
            .await
            .map_err(|e| map_error(&e))?;
        Ok(names
            .into_iter()
            .map(|name| SchemaInfo {
                system: SYSTEM_SCHEMAS.contains(&name.to_ascii_lowercase().as_str()),
                name,
            })
            .collect())
    }

    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        let rows: Vec<(String, String)> = self
            .conn
            .lock()
            .await
            .exec(
                "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
                (schema,),
            )
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .into_iter()
            .map(|(name, kind)| TableInfo {
                schema: schema.to_string(),
                name,
                kind: if kind.contains("VIEW") {
                    TableKind::View
                } else {
                    TableKind::Table
                },
            })
            .collect())
    }

    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let rows: Vec<(String, String, String, Option<String>, u32)> = self
            .conn
            .lock()
            .await
            .exec(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, ORDINAL_POSITION \
                 FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
                (&table.schema, &table.name),
            )
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .into_iter()
            .map(|(name, type_name, nullable, default, ordinal)| ColumnInfo {
                name,
                type_name,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                default,
                ordinal,
            })
            .collect())
    }

    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>> {
        let mut conn = self.conn.lock().await;
        // 主キー、一意キー、外部キーの列(順序つき)と、外部キーの参照先
        let rows: Vec<(String, String, String, Option<String>, Option<String>, Option<String>)> =
            conn.exec(
                "SELECT tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, k.COLUMN_NAME, \
                        k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME \
                 FROM information_schema.TABLE_CONSTRAINTS tc \
                 JOIN information_schema.KEY_COLUMN_USAGE k \
                   ON k.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA \
                  AND k.CONSTRAINT_NAME = tc.CONSTRAINT_NAME \
                  AND k.TABLE_NAME = tc.TABLE_NAME \
                 WHERE tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? \
                   AND tc.CONSTRAINT_TYPE IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY') \
                 ORDER BY tc.CONSTRAINT_NAME, k.ORDINAL_POSITION",
                (&table.schema, &table.name),
            )
            .await
            .map_err(|e| map_error(&e))?;
        let mut keys: Vec<KeyInfo> = Vec::new();
        let mut refs: Vec<(String, Vec<String>)> = Vec::new(); // 外部キーの参照先の列
        let mut targets: Vec<(String, String)> = Vec::new();
        for (name, kind, column, ref_schema, ref_table, ref_column) in rows {
            let is_new = keys.last().is_none_or(|k| k.name != name);
            if is_new {
                let kind = match kind.as_str() {
                    "PRIMARY KEY" => KeyKind::Primary,
                    "UNIQUE" => KeyKind::Unique,
                    _ => KeyKind::Foreign,
                };
                keys.push(KeyInfo {
                    name: name.clone(),
                    kind,
                    columns: Vec::new(),
                    definition: None,
                });
                refs.push((name.clone(), Vec::new()));
                targets.push((
                    ref_schema.unwrap_or_default(),
                    ref_table.unwrap_or_default(),
                ));
            }
            keys.last_mut().expect("just pushed").columns.push(column);
            if let Some(c) = ref_column {
                refs.last_mut().expect("just pushed").1.push(c);
            }
        }
        for ((key, (_, ref_cols)), (ref_schema, ref_table)) in
            keys.iter_mut().zip(&refs).zip(&targets)
        {
            if key.kind == KeyKind::Foreign {
                key.definition = Some(format!(
                    "FOREIGN KEY ({}) REFERENCES {}.{} ({})",
                    join_quoted(&key.columns),
                    quote(ref_schema),
                    quote(ref_table),
                    join_quoted(ref_cols),
                ));
            }
        }

        // CHECK 制約(MySQL 8.0.16 以降、MariaDB 10.2 以降)。無い版では空にする
        let checks: Vec<(String, String)> = conn
            .exec(
                "SELECT tc.CONSTRAINT_NAME, cc.CHECK_CLAUSE \
                 FROM information_schema.TABLE_CONSTRAINTS tc \
                 JOIN information_schema.CHECK_CONSTRAINTS cc \
                   ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA \
                  AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME \
                 WHERE tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? AND tc.CONSTRAINT_TYPE = 'CHECK' \
                 ORDER BY tc.CONSTRAINT_NAME",
                (&table.schema, &table.name),
            )
            .await
            .unwrap_or_default();
        keys.extend(checks.into_iter().map(|(name, clause)| KeyInfo {
            name,
            kind: KeyKind::Check,
            columns: Vec::new(),
            definition: Some(format!("CHECK {clause}")),
        }));
        Ok(keys)
    }

    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>> {
        let rows: Vec<(String, u8, String, String)> = self
            .conn
            .lock()
            .await
            .exec(
                "SELECT INDEX_NAME, NON_UNIQUE, INDEX_TYPE, COALESCE(COLUMN_NAME, '') \
                 FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX",
                (&table.schema, &table.name),
            )
            .await
            .map_err(|e| map_error(&e))?;
        let mut out: Vec<IndexInfo> = Vec::new();
        for (name, non_unique, method, column) in rows {
            if out.last().is_none_or(|i| i.name != name) {
                out.push(IndexInfo {
                    name: name.clone(),
                    columns: Vec::new(),
                    unique: non_unique == 0,
                    method,
                    definition: String::new(),
                });
            }
            out.last_mut().expect("just pushed").columns.push(column);
        }
        let qualified = format!("{}.{}", quote(&table.schema), quote(&table.name));
        for i in &mut out {
            i.definition = format!(
                "CREATE {}INDEX {} ON {} ({}) USING {}",
                if i.unique { "UNIQUE " } else { "" },
                quote(&i.name),
                qualified,
                join_quoted(&i.columns),
                i.method,
            );
        }
        Ok(out)
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let _ = self.conn.into_inner().disconnect().await;
        Ok(())
    }
}

fn join_quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(", ")
}
