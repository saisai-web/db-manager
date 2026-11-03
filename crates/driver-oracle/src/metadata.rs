//! スキーマ情報。`ALL_*` ビューから読む(権限のある範囲だけが見える)。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use driver_api::{
    ColumnInfo, ConnectionConfig, DriverError, ErrorKind, IndexInfo, KeyInfo, KeyKind,
    MetadataProvider, Result, SchemaInfo, TableInfo, TableKind, TableRef,
};
use oracledb::{Connection, Row, ToDbValue};

use crate::error::map_error;
use crate::open_blocking;

pub(crate) struct OracleMetadata {
    conn: Arc<Mutex<Connection>>,
}

impl OracleMetadata {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let cfg = cfg.clone();
        let conn = tokio::task::spawn_blocking(move || open_blocking(&cfg))
            .await
            .map_err(|e| DriverError::new(ErrorKind::Other, e.to_string()))??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 同期の API なので、専用の実行で、問い合わせる。
    async fn query(&self, sql: &'static str, binds: Vec<String>) -> Result<Vec<Row>> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|_| DriverError::new(ErrorKind::Other, "接続の排他に失敗しました"))?;
            let refs: Vec<&dyn ToDbValue> = binds.iter().map(|b| b as &dyn ToDbValue).collect();
            conn.query(sql, &refs)
                .map_err(|e| map_error(&e, sql))?
                .map(|r| r.map_err(|e| map_error(&e, sql)))
                .collect::<Result<Vec<Row>>>()
        })
        .await
        .map_err(|e| DriverError::new(ErrorKind::Other, e.to_string()))?
    }
}

fn text(row: &Row, i: usize) -> Result<String> {
    Ok(opt(row, i)?.unwrap_or_default())
}

fn opt(row: &Row, i: usize) -> Result<Option<String>> {
    row.get::<Option<String>>(i).map_err(|e| map_error(&e, ""))
}

fn int(row: &Row, i: usize) -> Result<Option<i64>> {
    row.get::<Option<i64>>(i).map_err(|e| map_error(&e, ""))
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn join_quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| quote(n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `CREATE TABLE` に書く形の型名。
fn type_name(
    data_type: &str,
    length: Option<i64>,
    char_length: Option<i64>,
    char_used: Option<&str>,
    precision: Option<i64>,
    scale: Option<i64>,
) -> String {
    match data_type {
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => {
            let unit = match (data_type, char_used) {
                ("VARCHAR2" | "CHAR", Some("C")) => " CHAR",
                _ => "",
            };
            format!("{data_type}({}{unit})", char_length.or(length).unwrap_or(0))
        }
        "RAW" => format!("RAW({})", length.unwrap_or(0)),
        "NUMBER" => match (precision, scale) {
            (None, _) => "NUMBER".into(),
            (Some(p), None | Some(0)) => format!("NUMBER({p})"),
            (Some(p), Some(s)) => format!("NUMBER({p},{s})"),
        },
        "FLOAT" => match precision {
            Some(p) => format!("FLOAT({p})"),
            None => "FLOAT".into(),
        },
        other => other.to_string(),
    }
}

#[async_trait]
impl MetadataProvider for OracleMetadata {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>> {
        let rows = self
            .query(
                "SELECT username, oracle_maintained FROM all_users ORDER BY username",
                vec![],
            )
            .await?;
        rows.iter()
            .map(|r| {
                Ok(SchemaInfo {
                    name: text(r, 0)?,
                    system: text(r, 1)? == "Y",
                })
            })
            .collect()
    }

    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>> {
        let rows = self
            .query(
                "SELECT table_name, 'TABLE' AS kind FROM all_tables \
                   WHERE owner = :1 AND nested = 'NO' AND secondary = 'N' \
                     AND (iot_type IS NULL OR iot_type = 'IOT') AND table_name NOT LIKE 'BIN$%' \
                     AND table_name NOT IN (SELECT mview_name FROM all_mviews WHERE owner = :2) \
                 UNION ALL SELECT view_name, 'VIEW' FROM all_views WHERE owner = :3 \
                 UNION ALL SELECT mview_name, 'MVIEW' FROM all_mviews WHERE owner = :4 \
                 ORDER BY 1",
                vec![schema.to_string(); 4],
            )
            .await?;
        rows.iter()
            .map(|r| {
                Ok(TableInfo {
                    schema: schema.to_string(),
                    name: text(r, 0)?,
                    kind: match text(r, 1)?.as_str() {
                        "VIEW" => TableKind::View,
                        "MVIEW" => TableKind::MaterializedView,
                        _ => TableKind::Table,
                    },
                })
            })
            .collect()
    }

    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>> {
        let rows = self
            .query(
                "SELECT column_name, data_type, data_length, char_length, char_used, \
                        data_precision, data_scale, nullable, data_default, column_id \
                 FROM all_tab_columns WHERE owner = :1 AND table_name = :2 \
                 ORDER BY column_id",
                vec![table.schema.clone(), table.name.clone()],
            )
            .await?;
        rows.iter()
            .map(|r| {
                let char_used = opt(r, 4)?;
                Ok(ColumnInfo {
                    name: text(r, 0)?,
                    type_name: type_name(
                        &text(r, 1)?,
                        int(r, 2)?,
                        int(r, 3)?.filter(|n| *n > 0),
                        char_used.as_deref(),
                        int(r, 5)?,
                        int(r, 6)?,
                    ),
                    nullable: text(r, 7)? == "Y",
                    default: opt(r, 8)?
                        .map(|d| d.trim().to_string())
                        .filter(|d| !d.is_empty()),
                    ordinal: int(r, 9)?.unwrap_or(0) as u32,
                })
            })
            .collect()
    }

    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>> {
        // 列は順序つき。外部キーは、参照先の表と列も、同じ位置で引く
        let rows = self
            .query(
                "SELECT c.constraint_name, c.constraint_type, cc.column_name, \
                        r.owner, r.table_name, rc.column_name, c.search_condition_vc, c.generated \
                 FROM all_constraints c \
                 LEFT JOIN all_cons_columns cc \
                   ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
                 LEFT JOIN all_constraints r \
                   ON r.owner = c.r_owner AND r.constraint_name = c.r_constraint_name \
                 LEFT JOIN all_cons_columns rc \
                   ON rc.owner = r.owner AND rc.constraint_name = r.constraint_name \
                  AND rc.position = cc.position \
                 WHERE c.owner = :1 AND c.table_name = :2 AND c.constraint_type IN ('P','U','R','C') \
                 ORDER BY c.constraint_name, cc.position",
                vec![table.schema.clone(), table.name.clone()],
            )
            .await?;
        let mut keys: Vec<KeyInfo> = Vec::new();
        let mut targets: Vec<(String, String, Vec<String>)> = Vec::new();
        for r in &rows {
            let name = text(r, 0)?;
            let ctype = text(r, 1)?;
            let condition = opt(r, 6)?;
            // NOT NULL は、列の属性として別に見える。名前が自動の NOT NULL は、制約として出さない
            if ctype == "C"
                && text(r, 7)? == "GENERATED NAME"
                && condition
                    .as_deref()
                    .is_some_and(|c| c.trim_end().ends_with("IS NOT NULL"))
            {
                continue;
            }
            if keys.last().is_none_or(|k| k.name != name) {
                keys.push(KeyInfo {
                    name: name.clone(),
                    kind: match ctype.as_str() {
                        "P" => KeyKind::Primary,
                        "U" => KeyKind::Unique,
                        "R" => KeyKind::Foreign,
                        _ => KeyKind::Check,
                    },
                    columns: Vec::new(),
                    definition: (ctype == "C")
                        .then(|| format!("CHECK ({})", condition.clone().unwrap_or_default())),
                });
                targets.push((text(r, 3)?, text(r, 4)?, Vec::new()));
            }
            if let Some(c) = opt(r, 2)? {
                keys.last_mut().expect("just pushed").columns.push(c);
            }
            if let Some(c) = opt(r, 5)? {
                targets.last_mut().expect("just pushed").2.push(c);
            }
        }
        for (key, (owner, name, cols)) in keys.iter_mut().zip(&targets) {
            if key.kind == KeyKind::Foreign {
                key.definition = Some(format!(
                    "FOREIGN KEY ({}) REFERENCES {}.{} ({})",
                    join_quoted(&key.columns),
                    quote(owner),
                    quote(name),
                    join_quoted(cols)
                ));
            }
        }
        Ok(keys)
    }

    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>> {
        let rows = self
            .query(
                "SELECT i.index_name, i.uniqueness, i.index_type, ic.column_name \
                 FROM all_indexes i JOIN all_ind_columns ic \
                   ON ic.index_owner = i.owner AND ic.index_name = i.index_name \
                 WHERE i.table_owner = :1 AND i.table_name = :2 \
                 ORDER BY i.index_name, ic.column_position",
                vec![table.schema.clone(), table.name.clone()],
            )
            .await?;
        let mut out: Vec<IndexInfo> = Vec::new();
        for r in &rows {
            let name = text(r, 0)?;
            if out.last().is_none_or(|i| i.name != name) {
                out.push(IndexInfo {
                    name,
                    columns: Vec::new(),
                    unique: text(r, 1)? == "UNIQUE",
                    method: text(r, 2)?,
                    definition: String::new(),
                });
            }
            out.last_mut()
                .expect("just pushed")
                .columns
                .push(text(r, 3)?);
        }
        let qualified = format!("{}.{}", quote(&table.schema), quote(&table.name));
        for i in &mut out {
            i.definition = format!(
                "CREATE {}INDEX {}.{} ON {} ({})",
                if i.unique { "UNIQUE " } else { "" },
                quote(&table.schema),
                quote(&i.name),
                qualified,
                join_quoted(&i.columns),
            );
        }
        Ok(out)
    }

    async fn close(self: Box<Self>) -> Result<()> {
        Ok(()) // 接続は、最後の参照が無くなったときに閉じる
    }
}

#[cfg(test)]
mod tests {
    use super::type_name;

    #[test]
    fn type_names_are_written_like_create_table() {
        assert_eq!(
            type_name("VARCHAR2", Some(80), Some(20), Some("C"), None, None),
            "VARCHAR2(20 CHAR)"
        );
        assert_eq!(
            type_name("VARCHAR2", Some(20), Some(20), Some("B"), None, None),
            "VARCHAR2(20)"
        );
        assert_eq!(
            type_name("NUMBER", Some(22), None, None, None, None),
            "NUMBER"
        );
        assert_eq!(
            type_name("NUMBER", Some(22), None, None, Some(10), Some(0)),
            "NUMBER(10)"
        );
        assert_eq!(
            type_name("NUMBER", Some(22), None, None, Some(12), Some(2)),
            "NUMBER(12,2)"
        );
        assert_eq!(
            type_name("RAW", Some(16), None, None, None, None),
            "RAW(16)"
        );
        assert_eq!(
            type_name("TIMESTAMP(6)", Some(11), None, None, None, Some(6)),
            "TIMESTAMP(6)"
        );
        assert_eq!(type_name("DATE", Some(7), None, None, None, None), "DATE");
    }
}
