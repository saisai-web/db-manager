//! 実際の DB の表の情報(スキーマ情報)から、ER 設計を作る(リバースエンジニアリング)。
//! 設計に取り込めない物(他のスキーマへの参照、CHECK 制約など)は、含めない。

use std::collections::HashSet;

use driver_api::KeyKind;
use er_design::{Column, DataType, Design, ForeignKey, Index, RefAction, Table, UniqueKey};

use crate::er::parse_reference;
use crate::types::TableDetails;

/// 設計の下書きの保存先(アプリのデータ保存先の `er-draft.erd`)。保存先を決められなければ `None`。
pub fn design_draft_path() -> Option<std::path::PathBuf> {
    std::env::var_os("SQLTOOL_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(storage::default_data_dir)
        .map(|d| d.join("er-draft.erd"))
}

/// `nextval('seq'::regclass)` は自動採番。`'A'::bpchar` のような型の指定は外す。
fn default_of(raw: &str) -> (Option<String>, bool) {
    let d = raw.trim();
    if d.to_ascii_lowercase().starts_with("nextval(") {
        return (None, true);
    }
    if d.starts_with('\'')
        && let Some(end) = d.rfind("'::")
    {
        return (Some(d[..=end].to_string()), false);
    }
    ((!d.is_empty()).then(|| d.to_string()), false)
}

fn action(definition: &str, which: &str) -> RefAction {
    let upper = definition.to_ascii_uppercase();
    let Some(at) = upper.find(&format!("ON {which} ")) else {
        return RefAction::NoAction;
    };
    let rest = &upper[at + 4 + which.len()..];
    if rest.starts_with("CASCADE") {
        RefAction::Cascade
    } else if rest.starts_with("SET NULL") {
        RefAction::SetNull
    } else if rest.starts_with("SET DEFAULT") {
        RefAction::SetDefault
    } else if rest.starts_with("RESTRICT") {
        RefAction::Restrict
    } else {
        RefAction::NoAction
    }
}

fn convert(d: &TableDetails) -> Table {
    let mut t = Table::new(d.table.name.clone());
    let key_columns = |kind: KeyKind| -> HashSet<&str> {
        d.keys
            .iter()
            .filter(|k| k.kind == kind)
            .flat_map(|k| k.columns.iter().map(String::as_str))
            .collect()
    };
    let primary = key_columns(KeyKind::Primary);
    let single_unique: HashSet<&str> = d
        .keys
        .iter()
        .filter(|k| k.kind == KeyKind::Unique && k.columns.len() == 1)
        .map(|k| k.columns[0].as_str())
        .collect();
    for c in &d.columns {
        let (default, auto) = c.default.as_deref().map_or((None, false), default_of);
        let is_pk = primary.contains(c.name.as_str());
        t.columns.push(Column {
            nullable: c.nullable && !is_pk,
            primary: is_pk,
            unique: single_unique.contains(c.name.as_str()) && !(is_pk && primary.len() == 1),
            auto,
            default,
            ..Column::new(c.name.clone(), DataType::parse(&c.type_name))
        });
    }
    let key_names: HashSet<&str> = d.keys.iter().map(|k| k.name.as_str()).collect();
    for k in d
        .keys
        .iter()
        .filter(|k| k.kind == KeyKind::Unique && k.columns.len() > 1)
    {
        t.uniques.push(UniqueKey {
            name: k.name.clone(),
            columns: k.columns.clone(),
        });
    }
    for i in &d.indexes {
        let pk = t.primary_key();
        if key_names.contains(i.name.as_str()) || i.columns == pk {
            continue;
        }
        if i.unique && i.columns.len() == 1 && single_unique.contains(i.columns[0].as_str()) {
            continue;
        }
        t.indexes.push(Index {
            name: i.name.clone(),
            columns: i.columns.clone(),
            unique: i.unique,
        });
    }
    t
}

/// 表の詳細から設計を作る。外部キーは、取り込んだ表どうしの関係だけ残す。
pub fn design_from_details<'a>(
    title: &str,
    details: impl IntoIterator<Item = &'a TableDetails>,
) -> Design {
    let details: Vec<&TableDetails> = details.into_iter().collect();
    let mut design = Design {
        title: title.to_string(),
        tables: details.iter().map(|d| convert(d)).collect(),
        ..Design::default()
    };
    design.tables.sort_by(|a, b| a.name.cmp(&b.name));
    for d in &details {
        for k in d.keys.iter().filter(|k| k.kind == KeyKind::Foreign) {
            let Some(def) = k.definition.as_deref() else {
                continue;
            };
            let Some((schema, ref_table, ref_columns)) = parse_reference(def) else {
                continue;
            };
            if schema.is_some_and(|s| s != d.table.schema) || design.table(&ref_table).is_none() {
                continue;
            }
            let Some(i) = design.table_index(&d.table.name) else {
                continue;
            };
            let mut fk = ForeignKey {
                name: k.name.clone(),
                columns: k.columns.clone(),
                ref_table,
                ref_columns,
                on_delete: action(def, "DELETE"),
                on_update: action(def, "UPDATE"),
            };
            let t = &mut design.tables[i];
            if t.fk_name(&ForeignKey {
                name: String::new(),
                ..fk.clone()
            }) == fk.name
            {
                fk.name.clear(); // 自動で付く名前と同じなら、指定なしにする
            }
            t.foreign_keys.push(fk);
        }
    }
    design
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::{ColumnInfo, IndexInfo, KeyInfo, TableRef};

    fn col(name: &str, ty: &str, nullable: bool, default: Option<&str>) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_name: ty.into(),
            nullable,
            default: default.map(String::from),
            ordinal: 1,
        }
    }

    fn key(name: &str, kind: KeyKind, cols: &[&str], def: Option<&str>) -> KeyInfo {
        KeyInfo {
            name: name.into(),
            kind,
            columns: cols.iter().map(|s| s.to_string()).collect(),
            definition: def.map(String::from),
        }
    }

    fn index(name: &str, cols: &[&str], unique: bool) -> IndexInfo {
        IndexInfo {
            name: name.into(),
            columns: cols.iter().map(|s| s.to_string()).collect(),
            unique,
            method: "btree".into(),
            definition: String::new(),
        }
    }

    fn sample() -> Vec<TableDetails> {
        vec![
            TableDetails {
                table: TableRef::new("public", "orders"),
                columns: vec![
                    col(
                        "id",
                        "integer",
                        false,
                        Some("nextval('orders_id_seq'::regclass)"),
                    ),
                    col("customer_id", "integer", false, None),
                    col("status", "character(1)", false, Some("'N'::bpchar")),
                    col("placed", "timestamp without time zone", true, Some("now()")),
                    col("note", "text", true, None),
                ],
                keys: vec![
                    key("orders_pkey", KeyKind::Primary, &["id"], None),
                    key(
                        "orders_customer_id_fkey",
                        KeyKind::Foreign,
                        &["customer_id"],
                        Some(
                            "FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE",
                        ),
                    ),
                    key(
                        "fk_to_other_schema",
                        KeyKind::Foreign,
                        &["note"],
                        Some("FOREIGN KEY (note) REFERENCES other.things(id)"),
                    ),
                    key("orders_check", KeyKind::Check, &[], Some("CHECK (id > 0)")),
                ],
                indexes: vec![
                    index("orders_pkey", &["id"], true),
                    index("orders_customer_idx", &["customer_id", "placed"], false),
                ],
            },
            TableDetails {
                table: TableRef::new("public", "customers"),
                columns: vec![
                    col("id", "integer", false, None),
                    col("email", "character varying(255)", false, None),
                ],
                keys: vec![
                    key("customers_pkey", KeyKind::Primary, &["id"], None),
                    key("customers_email_key", KeyKind::Unique, &["email"], None),
                ],
                indexes: vec![
                    index("customers_pkey", &["id"], true),
                    index("customers_email_key", &["email"], true),
                ],
            },
        ]
    }

    #[test]
    fn a_live_schema_becomes_a_design() {
        let d = design_from_details("shop", &sample());
        assert_eq!(d.title, "shop");
        assert_eq!(
            d.tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["customers", "orders"]
        );
        let o = d.table("orders").unwrap();
        let id = o.column("id").unwrap();
        assert!(id.primary && id.auto && !id.nullable && id.default.is_none());
        assert_eq!(id.ty, DataType::Int);
        assert_eq!(o.column("status").unwrap().default.as_deref(), Some("'N'"));
        assert_eq!(o.column("status").unwrap().ty, DataType::Char(Some(1)));
        assert_eq!(o.column("placed").unwrap().ty, DataType::DateTime);
        assert_eq!(
            o.column("placed").unwrap().default.as_deref(),
            Some("now()")
        );

        let c = d.table("customers").unwrap();
        assert!(c.column("email").unwrap().unique);
        assert_eq!(c.column("email").unwrap().ty, DataType::Varchar(Some(255)));
        assert!(c.indexes.is_empty(), "indexes behind keys are not repeated");
        assert_eq!(o.indexes.len(), 1);
        assert_eq!(o.indexes[0].columns, ["customer_id", "placed"]);
    }

    #[test]
    fn foreign_keys_keep_their_actions_and_drop_references_outside_the_schema() {
        let d = design_from_details("", &sample());
        let o = d.table("orders").unwrap();
        assert_eq!(o.foreign_keys.len(), 1, "other.things is not in the design");
        let fk = &o.foreign_keys[0];
        assert_eq!(
            (fk.ref_table.as_str(), fk.on_delete),
            ("customers", RefAction::Cascade)
        );
        assert_eq!(fk.name, "orders_customer_id_fkey", "a custom name is kept");
    }

    #[test]
    fn a_default_foreign_key_name_is_left_empty_and_the_design_round_trips_through_text() {
        let mut details = sample();
        details[0].keys[1].name = "fk_orders_customer_id".into();
        let d = design_from_details("", &details);
        assert!(d.table("orders").unwrap().foreign_keys[0].name.is_empty());
        let text = er_design::dsl::print(&d);
        assert_eq!(er_design::dsl::parse(&text).unwrap(), d, "{text}");
        assert!(
            er_design::validate(&d)
                .iter()
                .all(|i| i.severity != er_design::Severity::Error)
        );
    }

    #[test]
    fn an_empty_schema_gives_an_empty_design() {
        assert!(
            design_from_details("x", &Vec::<TableDetails>::new())
                .tables
                .is_empty()
        );
    }
}
