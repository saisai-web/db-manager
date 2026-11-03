use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use driver_api::{
    ColumnInfo, DriverError, ErrorKind, IndexInfo, KeyInfo, KeyKind, MetadataProvider, Result,
    SchemaInfo, TableInfo, TableKind, TableRef,
};
use rusqlite::Connection;

use crate::error::map_error;

/// スキーマ情報の取得用の接続。
pub(crate) struct SqliteMetadata {
    conn: Arc<Mutex<Connection>>,
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

impl SqliteMetadata {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| map_error(e, ""))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 同期の問い合わせを、専用のスレッドで行う。
    async fn with<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<R> + Send + 'static,
    ) -> Result<R> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap_or_else(|e| e.into_inner());
            f(&c).map_err(|e| map_error(e, ""))
        })
        .await
        .map_err(|e| DriverError::new(ErrorKind::Other, e.to_string()))?
    }
}

#[async_trait]
impl MetadataProvider for SqliteMetadata {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>> {
        self.with(|c| {
            let mut stmt = c.prepare("PRAGMA database_list")?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names
                .into_iter()
                .map(|name| SchemaInfo {
                    system: name == "temp",
                    name,
                })
                .collect())
        })
        .await
    }

    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        let schema = schema.to_string();
        self.with(move |c| {
            let sql = format!(
                "SELECT name, type FROM {}.sqlite_master WHERE type IN ('table', 'view') \
                 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
                quote(&schema)
            );
            let mut stmt = c.prepare(&sql)?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.map(|r| {
                let (name, kind) = r?;
                Ok(TableInfo {
                    schema: schema.clone(),
                    name,
                    kind: if kind == "view" {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                })
            })
            .collect()
        })
        .await
    }

    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let t = table.clone();
        self.with(move |c| table_info(c, &t).map(|cols| cols.into_iter().map(|x| x.info).collect()))
            .await
    }

    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>> {
        let t = table.clone();
        self.with(move |c| {
            let mut keys = Vec::new();
            let mut pk: Vec<_> = table_info(c, &t)?
                .into_iter()
                .filter(|x| x.pk > 0)
                .collect();
            pk.sort_by_key(|x| x.pk);
            if !pk.is_empty() {
                keys.push(KeyInfo {
                    name: "PRIMARY KEY".into(),
                    kind: KeyKind::Primary,
                    columns: pk.into_iter().map(|x| x.info.name).collect(),
                    definition: None,
                });
            }
            for (name, _, origin, _) in index_list(c, &t)? {
                if origin == "u" {
                    keys.push(KeyInfo {
                        name: name.clone(),
                        kind: KeyKind::Unique,
                        columns: index_columns(c, &t.schema, &name)?,
                        definition: None,
                    });
                }
            }
            keys.extend(foreign_keys(c, &t)?);
            Ok(keys)
        })
        .await
    }

    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>> {
        let t = table.clone();
        self.with(move |c| {
            index_list(c, &t)?
                .into_iter()
                .map(|(name, unique, _, _)| {
                    let definition: Option<String> = c
                        .query_row(
                            &format!(
                                "SELECT sql FROM {}.sqlite_master WHERE name = ?1",
                                quote(&t.schema)
                            ),
                            [&name],
                            |r| r.get(0),
                        )
                        .ok()
                        .flatten();
                    Ok(IndexInfo {
                        columns: index_columns(c, &t.schema, &name)?,
                        name,
                        unique,
                        method: "btree".into(),
                        definition: definition.unwrap_or_default(),
                    })
                })
                .collect()
        })
        .await
    }

    async fn close(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

struct Col {
    info: ColumnInfo,
    pk: i64,
}

fn table_info(c: &Connection, t: &TableRef) -> rusqlite::Result<Vec<Col>> {
    let mut stmt = c.prepare(&format!(
        "PRAGMA {}.table_info({})",
        quote(&t.schema),
        quote(&t.name)
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok(Col {
            info: ColumnInfo {
                ordinal: r.get::<_, i64>(0)? as u32 + 1,
                name: r.get(1)?,
                type_name: r.get::<_, String>(2)?,
                nullable: r.get::<_, i64>(3)? == 0,
                default: r.get(4)?,
            },
            pk: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// (名前, 一意か, 由来: c=CREATE INDEX / u=UNIQUE 制約 / pk=主キー, 部分索引か)
fn index_list(c: &Connection, t: &TableRef) -> rusqlite::Result<Vec<(String, bool, String, bool)>> {
    let mut stmt = c.prepare(&format!(
        "PRAGMA {}.index_list({})",
        quote(&t.schema),
        quote(&t.name)
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get(1)?,
            r.get::<_, i64>(2)? != 0,
            r.get(3)?,
            r.get::<_, i64>(4)? != 0,
        ))
    })?;
    rows.collect()
}

fn index_columns(c: &Connection, schema: &str, index: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = c.prepare(&format!(
        "PRAGMA {}.index_info({})",
        quote(schema),
        quote(index)
    ))?;
    let rows = stmt.query_map([], |r| r.get::<_, Option<String>>(2))?;
    rows.map(|n| Ok(n?.unwrap_or_else(|| "(式)".into())))
        .collect()
}

fn foreign_keys(c: &Connection, t: &TableRef) -> rusqlite::Result<Vec<KeyInfo>> {
    let mut stmt = c.prepare(&format!(
        "PRAGMA {}.foreign_key_list({})",
        quote(&t.schema),
        quote(&t.name)
    ))?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut by_id: Vec<(i64, String, Vec<String>, Vec<String>)> = Vec::new();
    for row in rows {
        let (id, table, from, to) = row?;
        match by_id.iter_mut().find(|(i, ..)| *i == id) {
            Some((_, _, f, t)) => {
                f.push(from);
                t.push(to.unwrap_or_default());
            }
            None => by_id.push((id, table, vec![from], vec![to.unwrap_or_default()])),
        }
    }
    Ok(by_id
        .into_iter()
        .map(|(id, table, from, to)| KeyInfo {
            name: format!("fk_{id}"),
            kind: KeyKind::Foreign,
            definition: Some(format!("REFERENCES {table}({})", to.join(", "))),
            columns: from,
        })
        .collect())
}
