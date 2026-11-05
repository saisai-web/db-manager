//! 表のほかの文: スキーマ、DROP、索引、コメント、名前の長さの点検。

use super::names::{describe, ident, ident_list, literal, qualified};
use super::order::Order;
use super::{DdlOptions, Target, schema_for};
use crate::model::{Design, Table};

pub fn create_schema(target: Target, opts: &DdlOptions) -> Option<String> {
    (target == Target::Postgres && !opts.schema.is_empty()).then(|| {
        format!(
            "CREATE SCHEMA IF NOT EXISTS {};",
            ident(target, &opts.schema)
        )
    })
}

/// 参照される側(親)を最後に DROP する。
pub fn drops(d: &Design, order: &Order, target: Target, opts: &DdlOptions) -> Vec<String> {
    let mut out = Vec::new();
    if target == Target::MySql && order.has_cycle {
        out.push("SET FOREIGN_KEY_CHECKS = 0;".to_string());
    }
    if target == Target::SqlServer {
        out.extend(sqlserver_fk_drops(d, order, opts));
    }
    for &i in order.tables.iter().rev() {
        let name = qualified(target, schema_for(target, opts), &d.tables[i].name);
        out.push(match target {
            Target::Postgres => format!("DROP TABLE IF EXISTS {name} CASCADE;"),
            Target::MySql | Target::Sqlite | Target::SqlServer => {
                format!("DROP TABLE IF EXISTS {name};")
            }
            // Oracle には IF EXISTS がない(表がなくてもエラーにしない)
            Target::Oracle => format!(
                "BEGIN\n  EXECUTE IMMEDIATE 'DROP TABLE {} CASCADE CONSTRAINTS';\nEXCEPTION WHEN OTHERS THEN\n  IF SQLCODE != -942 THEN RAISE; END IF;\nEND;\n/",
                name.replace('\'', "''")
            ),
        });
    }
    if target == Target::MySql && order.has_cycle {
        out.push("SET FOREIGN_KEY_CHECKS = 1;".to_string());
    }
    out
}

/// SQL Server は、ほかの表から外部キーで参照されている表を DROP できない。循環する参照
/// (全表を作ったあとに足す外部キー)は、表を落とす前に外部キーだけ先に外す。
fn sqlserver_fk_drops(d: &Design, order: &Order, opts: &DdlOptions) -> Vec<String> {
    let target = Target::SqlServer;
    let mut created = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &i in &order.tables {
        let t = &d.tables[i];
        for fk in &t.foreign_keys {
            let deferred = fk.ref_table != t.name
                && d.table(&fk.ref_table).is_some()
                && !created.contains(fk.ref_table.as_str());
            if deferred {
                let name = t.fk_name(fk);
                let full = if opts.schema.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{name}", opts.schema)
                };
                out.push(format!(
                    "IF OBJECT_ID({}, N'F') IS NOT NULL\n  ALTER TABLE {} DROP CONSTRAINT {};",
                    literal(target, &full),
                    qualified(target, &opts.schema, &t.name),
                    ident(target, &name)
                ));
            }
        }
        created.insert(t.name.as_str());
    }
    out
}

pub fn indexes(d: &Design, t: &Table, target: Target, opts: &DdlOptions) -> Vec<String> {
    let table = qualified(target, schema_for(target, opts), &t.name);
    let mut out: Vec<String> = t
        .indexes
        .iter()
        .map(|i| {
            format!(
                "CREATE {}INDEX {} ON {table} ({});",
                if i.unique { "UNIQUE " } else { "" },
                ident(target, &t.index_name(i)),
                ident_list(target, &i.columns)
            )
        })
        .collect();
    if opts.index_foreign_keys && target != Target::MySql {
        let covered = |cols: &[String]| {
            let starts = |have: &[String]| have.len() >= cols.len() && have[..cols.len()] == *cols;
            starts(&t.primary_key())
                || t.uniques.iter().any(|u| starts(&u.columns))
                || t.indexes.iter().any(|i| starts(&i.columns))
        };
        let mut seen: Vec<&[String]> = Vec::new();
        for fk in t
            .foreign_keys
            .iter()
            .filter(|f| d.table(&f.ref_table).is_some())
        {
            if covered(&fk.columns) || seen.contains(&fk.columns.as_slice()) {
                continue;
            }
            seen.push(&fk.columns);
            let name = format!("ix_{}_{}", t.name, fk.columns.join("_"));
            out.push(format!(
                "CREATE INDEX {} ON {table} ({});",
                ident(target, &name),
                ident_list(target, &fk.columns)
            ));
        }
    }
    out
}

/// DB にコメントとして残す文(PostgreSQL・Oracle・SQL Server。MySQL は列定義の中、SQLite は SQL のコメント)。
pub fn comments(d: &Design, target: Target, opts: &DdlOptions) -> Vec<String> {
    let mut out = Vec::new();
    let schema = schema_for(target, opts);
    for t in &d.tables {
        let table = qualified(target, schema, &t.name);
        let mut push = |column: Option<&str>, text: String| {
            if text.is_empty() {
                return;
            }
            match target {
                Target::Postgres | Target::Oracle => out.push(match column {
                    None => format!("COMMENT ON TABLE {table} IS {};", literal(target, &text)),
                    Some(c) => format!(
                        "COMMENT ON COLUMN {table}.{} IS {};",
                        ident(target, c),
                        literal(target, &text)
                    ),
                }),
                Target::SqlServer => {
                    let schema = if schema.is_empty() { "dbo" } else { schema };
                    let level2 = column.map_or(String::new(), |c| {
                        format!(
                            ", @level2type = N'COLUMN', @level2name = {}",
                            literal(target, c)
                        )
                    });
                    out.push(format!(
                        "EXEC sys.sp_addextendedproperty @name = N'MS_Description', @value = {}, @level0type = N'SCHEMA', @level0name = {}, @level1type = N'TABLE', @level1name = {}{level2};",
                        literal(target, &text),
                        literal(target, schema),
                        literal(target, &t.name),
                    ));
                }
                Target::MySql | Target::Sqlite => {}
            }
        };
        push(None, describe(&t.logical, &t.comment));
        for c in &t.columns {
            push(Some(&c.name), describe(&c.logical, &c.comment));
        }
    }
    out
}

/// DB の名前の長さの上限(超えると、切り詰められるか、エラーになる)。
fn name_limit(target: Target) -> Option<usize> {
    match target {
        Target::Postgres => Some(63),
        Target::MySql => Some(64),
        Target::SqlServer | Target::Oracle => Some(128),
        Target::Sqlite => None,
    }
}

pub fn check_name_lengths(d: &Design, target: Target, w: &mut Vec<String>) {
    let Some(limit) = name_limit(target) else {
        return;
    };
    let long = |name: &str| name.len() > limit; // PostgreSQL の上限はバイト数
    for t in &d.tables {
        let names = std::iter::once(t.name.clone())
            .chain(t.columns.iter().map(|c| c.name.clone()))
            .chain(t.foreign_keys.iter().map(|f| t.fk_name(f)))
            .chain(t.indexes.iter().map(|i| t.index_name(i)))
            .chain(t.uniques.iter().map(|u| t.unique_name(u)));
        for n in names.filter(|n| long(n)) {
            w.push(format!(
                "{}: 名前 '{n}' は {limit} 文字(バイト)を超えています",
                t.name
            ));
        }
    }
}
