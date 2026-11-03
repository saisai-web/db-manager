use async_trait::async_trait;
use driver_api::{
    ColumnInfo, ConnectionConfig, IndexInfo, KeyInfo, KeyKind, MetadataProvider, Result,
    SchemaInfo, TableInfo, TableKind, TableRef,
};
use tokio::task::JoinHandle;
use tokio_postgres::Client;

use crate::error::map_error;

/// スキーマ情報の取得用の接続。`Session` とは別なので、実行中のクエリに待たされない。
pub(crate) struct PgMetadata {
    client: Client,
    task: JoinHandle<()>,
}

impl PgMetadata {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let mut cfg = cfg.clone();
        cfg.application_name = format!("{} (metadata)", cfg.application_name);
        let (client, task, _) = crate::open(&cfg).await?;
        Ok(Self { client, task })
    }
}

const SCHEMAS: &str = "
SELECT n.nspname::text,
       n.nspname IN ('pg_catalog', 'information_schema')
FROM pg_namespace n
WHERE n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'
ORDER BY 2, 1";

const TABLES: &str = "
SELECT c.relname::text, c.relkind::text
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
ORDER BY c.relname";

const COLUMNS: &str = "
SELECT a.attname::text,
       format_type(a.atttypid, a.atttypmod),
       NOT a.attnotnull,
       pg_get_expr(d.adbin, d.adrelid),
       a.attnum::int
FROM pg_attribute a
JOIN pg_class c ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped
ORDER BY a.attnum";

const KEYS: &str = "
SELECT k.conname::text,
       k.contype::text,
       ARRAY(SELECT a.attname::text
             FROM unnest(k.conkey) WITH ORDINALITY u(attnum, ord)
             JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = u.attnum
             ORDER BY u.ord),
       pg_get_constraintdef(k.oid)
FROM pg_constraint k
JOIN pg_class c ON c.oid = k.conrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = $1 AND c.relname = $2 AND k.contype IN ('p', 'u', 'f', 'c', 'x')
ORDER BY CASE k.contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, k.conname";

const INDEXES: &str = "
SELECT i.relname::text,
       ARRAY(SELECT pg_get_indexdef(i.oid, g, true) FROM generate_series(1, x.indnkeyatts) g),
       x.indisunique,
       am.amname::text,
       pg_get_indexdef(i.oid)
FROM pg_index x
JOIN pg_class i ON i.oid = x.indexrelid
JOIN pg_class c ON c.oid = x.indrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_am am ON am.oid = i.relam
WHERE n.nspname = $1 AND c.relname = $2
ORDER BY i.relname";

#[async_trait]
impl MetadataProvider for PgMetadata {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = self
            .client
            .query(SCHEMAS, &[])
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .iter()
            .map(|r| SchemaInfo {
                name: r.get(0),
                system: r.get(1),
            })
            .collect())
    }

    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        let rows = self
            .client
            .query(TABLES, &[&schema])
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .iter()
            .map(|r| {
                let relkind: String = r.get(1);
                TableInfo {
                    schema: schema.to_string(),
                    name: r.get(0),
                    kind: match relkind.as_str() {
                        "v" => TableKind::View,
                        "m" => TableKind::MaterializedView,
                        "f" => TableKind::ForeignTable,
                        _ => TableKind::Table,
                    },
                }
            })
            .collect())
    }

    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let rows = self
            .client
            .query(COLUMNS, &[&table.schema, &table.name])
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .iter()
            .map(|r| ColumnInfo {
                name: r.get(0),
                type_name: r.get(1),
                nullable: r.get(2),
                default: r.get(3),
                ordinal: r.get::<_, i32>(4) as u32,
            })
            .collect())
    }

    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>> {
        let rows = self
            .client
            .query(KEYS, &[&table.schema, &table.name])
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .iter()
            .map(|r| {
                let contype: String = r.get(1);
                KeyInfo {
                    name: r.get(0),
                    kind: match contype.as_str() {
                        "p" => KeyKind::Primary,
                        "u" => KeyKind::Unique,
                        "f" => KeyKind::Foreign,
                        "x" => KeyKind::Exclusion,
                        _ => KeyKind::Check,
                    },
                    columns: r.get(2),
                    definition: r.get(3),
                }
            })
            .collect())
    }

    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>> {
        let rows = self
            .client
            .query(INDEXES, &[&table.schema, &table.name])
            .await
            .map_err(|e| map_error(&e))?;
        Ok(rows
            .iter()
            .map(|r| IndexInfo {
                name: r.get(0),
                columns: r.get(1),
                unique: r.get(2),
                method: r.get(3),
                definition: r.get(4),
            })
            .collect())
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let Self { client, task } = *self;
        drop(client);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        Ok(())
    }
}
