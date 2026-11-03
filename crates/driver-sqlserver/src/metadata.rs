use async_trait::async_trait;
use driver_api::{
    ColumnInfo, ConnectionConfig, IndexInfo, KeyInfo, KeyKind, MetadataProvider, Result,
    SchemaInfo, TableInfo, TableKind, TableRef,
};
use tiberius::{Row, ToSql};
use tokio::sync::Mutex;

use crate::error::map_error;
use crate::values::cell;
use crate::{Conn, open};

/// 管理用のスキーマ。ツリーでは畳んで表示する。
const SYSTEM_SCHEMAS: [&str; 3] = ["sys", "INFORMATION_SCHEMA", "guest"];

pub(crate) struct SqlServerMetadata {
    conn: Mutex<Conn>,
}

impl SqlServerMetadata {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        Ok(Self {
            conn: Mutex::new(open(cfg).await?),
        })
    }

    async fn rows(&self, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>> {
        let mut conn = self.conn.lock().await;
        conn.query(sql, params)
            .await
            .map_err(|e| map_error(&e))?
            .into_first_result()
            .await
            .map_err(|e| map_error(&e))
    }
}

/// i-th セルの文字列(NULL は `None`)。列の型が何であっても読める。
fn text(row: &Row, i: usize) -> Option<String> {
    let (_, data) = row.cells().nth(i)?;
    cell(data).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn s(row: &Row, i: usize) -> String {
    text(row, i).unwrap_or_default()
}

fn n(row: &Row, i: usize) -> i64 {
    text(row, i).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn quote(ident: &str) -> String {
    format!("[{}]", ident.replace(']', "]]"))
}

fn join_quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `sys.columns` の情報から、`nvarchar(50)` や `decimal(20,6)` のような型の表記を作る。
fn type_display(name: &str, max_length: i64, precision: i64, scale: i64) -> String {
    match name {
        "nvarchar" | "nchar" => match max_length {
            -1 => format!("{name}(max)"),
            l => format!("{name}({})", l / 2),
        },
        "varchar" | "char" | "varbinary" | "binary" => match max_length {
            -1 => format!("{name}(max)"),
            l => format!("{name}({l})"),
        },
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{name}({scale})"),
        _ => name.to_string(),
    }
}

#[async_trait]
impl MetadataProvider for SqlServerMetadata {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = self
            .rows("SELECT name FROM sys.schemas ORDER BY name", &[])
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let name = s(r, 0);
                SchemaInfo {
                    system: SYSTEM_SCHEMAS.contains(&name.as_str()) || name.starts_with("db_"),
                    name,
                }
            })
            .collect())
    }

    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        let rows = self
            .rows(
                "SELECT o.name, o.type FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE s.name = @P1 AND o.type IN ('U', 'V') ORDER BY o.name",
                &[&schema],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TableInfo {
                schema: schema.to_string(),
                name: s(r, 0),
                kind: if s(r, 1).trim() == "V" {
                    TableKind::View
                } else {
                    TableKind::Table
                },
            })
            .collect())
    }

    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let rows = self
            .rows(
                "SELECT c.name, t.name, c.max_length, c.precision, c.scale, c.is_nullable, \
                        dc.definition, c.column_id \
                 FROM sys.columns c \
                 JOIN sys.objects o ON o.object_id = c.object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 JOIN sys.types t ON t.user_type_id = c.user_type_id \
                 LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id \
                 WHERE s.name = @P1 AND o.name = @P2 ORDER BY c.column_id",
                &[&table.schema.as_str(), &table.name.as_str()],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ColumnInfo {
                name: s(r, 0),
                type_name: type_display(&s(r, 1), n(r, 2), n(r, 3), n(r, 4)),
                nullable: n(r, 5) != 0,
                default: text(r, 6),
                ordinal: u32::try_from(n(r, 7)).unwrap_or(0),
            })
            .collect())
    }

    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>> {
        let p: [&dyn ToSql; 2] = [&table.schema.as_str(), &table.name.as_str()];
        // 主キーと一意制約: 列を順序つきで
        let rows = self
            .rows(
                "SELECT kc.name, kc.type, c.name \
                 FROM sys.key_constraints kc \
                 JOIN sys.objects o ON o.object_id = kc.parent_object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 JOIN sys.index_columns ic ON ic.object_id = kc.parent_object_id AND ic.index_id = kc.unique_index_id \
                 JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
                 WHERE s.name = @P1 AND o.name = @P2 AND ic.is_included_column = 0 \
                 ORDER BY kc.name, ic.key_ordinal",
                &p,
            )
            .await?;
        let mut keys: Vec<KeyInfo> = Vec::new();
        for r in &rows {
            let name = s(r, 0);
            if keys.last().is_none_or(|k| k.name != name) {
                keys.push(KeyInfo {
                    name,
                    kind: if s(r, 1).trim() == "PK" {
                        KeyKind::Primary
                    } else {
                        KeyKind::Unique
                    },
                    columns: Vec::new(),
                    definition: None,
                });
            }
            keys.last_mut().expect("just pushed").columns.push(s(r, 2));
        }

        // 外部キー: 列と、参照先
        let rows = self
            .rows(
                "SELECT fk.name, pc.name, rs.name, ro.name, rc.name \
                 FROM sys.foreign_keys fk \
                 JOIN sys.objects o ON o.object_id = fk.parent_object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
                 JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN sys.objects ro ON ro.object_id = fkc.referenced_object_id \
                 JOIN sys.schemas rs ON rs.schema_id = ro.schema_id \
                 JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                 WHERE s.name = @P1 AND o.name = @P2 ORDER BY fk.name, fkc.constraint_column_id",
                &p,
            )
            .await?;
        let mut fks: Vec<(KeyInfo, String, String, Vec<String>)> = Vec::new();
        for r in &rows {
            let name = s(r, 0);
            if fks.last().is_none_or(|(k, ..)| k.name != name) {
                fks.push((
                    KeyInfo {
                        name,
                        kind: KeyKind::Foreign,
                        columns: Vec::new(),
                        definition: None,
                    },
                    s(r, 2),
                    s(r, 3),
                    Vec::new(),
                ));
            }
            let last = fks.last_mut().expect("just pushed");
            last.0.columns.push(s(r, 1));
            last.3.push(s(r, 4));
        }
        keys.extend(fks.into_iter().map(|(mut k, schema, table, refs)| {
            k.definition = Some(format!(
                "FOREIGN KEY ({}) REFERENCES {}.{} ({})",
                join_quoted(&k.columns),
                quote(&schema),
                quote(&table),
                join_quoted(&refs)
            ));
            k
        }));

        // CHECK 制約
        let rows = self
            .rows(
                "SELECT cc.name, cc.definition FROM sys.check_constraints cc \
                 JOIN sys.objects o ON o.object_id = cc.parent_object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE s.name = @P1 AND o.name = @P2 ORDER BY cc.name",
                &p,
            )
            .await?;
        keys.extend(rows.iter().map(|r| KeyInfo {
            name: s(r, 0),
            kind: KeyKind::Check,
            columns: Vec::new(),
            definition: Some(format!("CHECK {}", s(r, 1))),
        }));
        Ok(keys)
    }

    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>> {
        let rows = self
            .rows(
                "SELECT i.name, i.is_unique, i.type_desc, c.name \
                 FROM sys.indexes i \
                 JOIN sys.objects o ON o.object_id = i.object_id \
                 JOIN sys.schemas s ON s.schema_id = o.schema_id \
                 JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                 JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
                 WHERE s.name = @P1 AND o.name = @P2 AND i.type > 0 AND ic.is_included_column = 0 \
                 ORDER BY i.name, ic.key_ordinal",
                &[&table.schema.as_str(), &table.name.as_str()],
            )
            .await?;
        let mut out: Vec<IndexInfo> = Vec::new();
        for r in &rows {
            let name = s(r, 0);
            if out.last().is_none_or(|i| i.name != name) {
                out.push(IndexInfo {
                    name,
                    columns: Vec::new(),
                    unique: n(r, 1) != 0,
                    method: s(r, 2),
                    definition: String::new(),
                });
            }
            out.last_mut().expect("just pushed").columns.push(s(r, 3));
        }
        let qualified = format!("{}.{}", quote(&table.schema), quote(&table.name));
        for i in &mut out {
            i.definition = format!(
                "CREATE {}{} INDEX {} ON {} ({})",
                if i.unique { "UNIQUE " } else { "" },
                i.method,
                quote(&i.name),
                qualified,
                join_quoted(&i.columns),
            );
        }
        Ok(out)
    }

    async fn close(self: Box<Self>) -> Result<()> {
        let _ = self.conn.into_inner().close().await;
        Ok(())
    }
}
