//! 設計書の表の行。HTML と Excel で同じ内容になるよう、文字列にしてここで作る。

use crate::model::{Design, Table};

pub struct ColumnRow {
    pub no: usize,
    pub logical: String,
    pub name: String,
    pub ty: String,
    pub not_null: bool,
    pub primary: bool,
    /// 外部キーなら、参照先(`customers.id`)
    pub foreign: String,
    pub unique: bool,
    pub auto: bool,
    pub default: String,
    pub comment: String,
}

pub fn column_rows(d: &Design, t: &Table) -> Vec<ColumnRow> {
    t.columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let foreign = t
                .foreign_keys
                .iter()
                .filter_map(|f| {
                    let at = f.columns.iter().position(|x| x == &c.name)?;
                    let refs = d.ref_columns(f);
                    Some(format!(
                        "{}.{}",
                        f.ref_table,
                        refs.get(at).map_or("?", String::as_str)
                    ))
                })
                .collect::<Vec<_>>()
                .join(", ");
            ColumnRow {
                no: i + 1,
                logical: c.logical.clone(),
                name: c.name.clone(),
                ty: c.ty.to_string(),
                not_null: !c.nullable || c.primary,
                primary: c.primary,
                foreign,
                unique: c.unique,
                auto: c.auto,
                default: c.default.clone().unwrap_or_default(),
                comment: c.comment.clone(),
            }
        })
        .collect()
}

pub struct ConstraintRow {
    pub kind: &'static str,
    pub name: String,
    pub columns: String,
    /// 外部キーなら参照先、索引・一意制約なら空
    pub target: String,
    pub on_delete: &'static str,
    pub on_update: &'static str,
}

/// 表の制約(主キー・一意・外部キー)と索引。
pub fn constraint_rows(d: &Design, t: &Table) -> Vec<ConstraintRow> {
    let plain = |kind, name: String, cols: &[String]| ConstraintRow {
        kind,
        name,
        columns: cols.join(", "),
        target: String::new(),
        on_delete: "",
        on_update: "",
    };
    let mut out = Vec::new();
    let pk = t.primary_key();
    if !pk.is_empty() {
        out.push(plain("主キー", t.pk_name(), &pk));
    }
    for c in t
        .columns
        .iter()
        .filter(|c| c.unique && pk != [c.name.clone()])
    {
        let u = crate::model::UniqueKey {
            name: String::new(),
            columns: vec![c.name.clone()],
        };
        out.push(plain("一意制約", t.unique_name(&u), &u.columns));
    }
    for u in &t.uniques {
        out.push(plain("一意制約", t.unique_name(u), &u.columns));
    }
    for f in &t.foreign_keys {
        out.push(ConstraintRow {
            kind: "外部キー",
            name: t.fk_name(f),
            columns: f.columns.join(", "),
            target: format!("{}({})", f.ref_table, d.ref_columns(f).join(", ")),
            on_delete: f.on_delete.label(),
            on_update: f.on_update.label(),
        });
    }
    for i in &t.indexes {
        out.push(plain(
            if i.unique { "一意索引" } else { "索引" },
            t.index_name(i),
            &i.columns,
        ));
    }
    out
}

pub struct RelationRow {
    pub parent: String,
    pub child: String,
    pub name: String,
    pub columns: String,
    /// `1 : 0..N` のように、親 : 子
    pub cardinality: String,
    pub on_delete: &'static str,
    pub on_update: &'static str,
}

pub fn relation_rows(d: &Design) -> Vec<RelationRow> {
    let mut out = Vec::new();
    for t in &d.tables {
        for f in &t.foreign_keys {
            if d.table(&f.ref_table).is_none() {
                continue;
            }
            let refs = d.ref_columns(f);
            let parent_side = if t.fk_required(f) { "1" } else { "0..1" };
            let child_side = if t.fk_is_one_to_one(f) {
                "0..1"
            } else {
                "0..N"
            };
            out.push(RelationRow {
                parent: f.ref_table.clone(),
                child: t.name.clone(),
                name: t.fk_name(f),
                columns: format!(
                    "{} → {}.{}",
                    f.columns.join(", "),
                    f.ref_table,
                    refs.join(", ")
                ),
                cardinality: format!("{parent_side} : {child_side}"),
                on_delete: f.on_delete.label(),
                on_update: f.on_update.label(),
            });
        }
    }
    out
}

/// この表を参照している表(子)の名前。
pub fn referenced_by(d: &Design, t: &Table) -> Vec<String> {
    let mut v: Vec<String> = d
        .tables
        .iter()
        .filter(|c| c.name != t.name && c.foreign_keys.iter().any(|f| f.ref_table == t.name))
        .map(|c| c.name.clone())
        .collect();
    v.dedup();
    v
}

/// この表が参照している表(親)の名前。
pub fn references(d: &Design, t: &Table) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for f in &t.foreign_keys {
        if f.ref_table != t.name && d.table(&f.ref_table).is_some() && !v.contains(&f.ref_table) {
            v.push(f.ref_table.clone());
        }
    }
    v
}

/// 今日の日付(`2026-11-04`)。OS の `date` コマンドで利用者の地域の日付を得る(使えなければ UTC)。
/// 外部のコマンドを呼ぶので、画面のスレッドでは呼ばないこと。
pub fn today() -> String {
    local_date().unwrap_or_else(utc_date)
}

fn local_date() -> Option<String> {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    let shaped = s.len() == 10
        && s.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    shaped.then(|| s.to_string())
}

fn utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    ymd(secs / 86_400)
}

/// 1970-01-01 からの日数を `YYYY-MM-DD` にする。
fn ymd(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    const SRC: &str = "customers \"顧客\"\n  id int pk auto \"ID\"\n  email varchar(255) unique default 'x' \"メール\" \"連絡用\"\norders\n  id int pk\n  customer_id int not null -> customers.id on delete cascade\n  note text\n  index (note)\n  unique uq_note (note)\n";

    #[test]
    fn column_rows_describe_keys_and_references() {
        let d = parse(SRC).unwrap();
        let rows = column_rows(&d, &d.tables[1]);
        assert_eq!(rows[1].foreign, "customers.id");
        assert!(rows[1].not_null && !rows[2].not_null && rows[0].primary);
        let c = column_rows(&d, &d.tables[0]);
        assert_eq!(
            (
                c[1].logical.as_str(),
                c[1].comment.as_str(),
                c[1].default.as_str()
            ),
            ("メール", "連絡用", "'x'")
        );
        assert!(c[0].auto && c[1].unique);
        assert_eq!(c[1].no, 2);
    }

    #[test]
    fn constraint_rows_list_keys_uniques_foreign_keys_and_indexes() {
        let d = parse(SRC).unwrap();
        let rows = constraint_rows(&d, &d.tables[1]);
        let kinds: Vec<_> = rows.iter().map(|r| (r.kind, r.name.as_str())).collect();
        assert_eq!(
            kinds,
            [
                ("主キー", "pk_orders"),
                ("一意制約", "uq_note"),
                ("外部キー", "fk_orders_customer_id"),
                ("索引", "ix_orders_note")
            ]
        );
        let fk = &rows[2];
        assert_eq!(
            (fk.target.as_str(), fk.on_delete, fk.on_update),
            ("customers(id)", "連鎖(CASCADE)", "なし")
        );
        let c = constraint_rows(&d, &d.tables[0]);
        assert_eq!(c[1].name, "uq_customers_email");
    }

    #[test]
    fn relation_rows_show_cardinality_from_parent_to_child() {
        let d = parse(SRC).unwrap();
        let rel = relation_rows(&d);
        assert_eq!(rel.len(), 1);
        assert_eq!(
            (rel[0].parent.as_str(), rel[0].child.as_str()),
            ("customers", "orders")
        );
        assert_eq!(rel[0].cardinality, "1 : 0..N");
        assert_eq!(rel[0].columns, "customer_id → customers.id");
        assert_eq!(referenced_by(&d, &d.tables[0]), ["orders"]);
        assert_eq!(references(&d, &d.tables[1]), ["customers"]);
    }

    #[test]
    fn dates_are_formatted_from_days_since_the_epoch() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(19_723), "2024-01-01");
        assert_eq!(ymd(20_516), "2026-03-04");
        assert_eq!(today().len(), 10);
        assert_eq!(utc_date().len(), 10);
    }
}
