//! 画面の操作に対応する、設計の変更。表を足す、2 つの表(列)を選んで関係をつくる。

use crate::model::{Column, Design, ForeignKey, Table};
use crate::types::DataType;

/// 関係をつくるときに選んだもの。列まで選んでいなければ、表だけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub table: String,
    pub column: Option<String>,
}

impl Pick {
    pub fn table(name: &str) -> Self {
        Self {
            table: name.into(),
            column: None,
        }
    }

    pub fn column(table: &str, column: &str) -> Self {
        Self {
            table: table.into(),
            column: Some(column.into()),
        }
    }
}

/// 英語の複数形を、ざっくり単数形にする(`customers` → `customer`、`categories` → `category`)。
pub fn singular(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies").filter(|s| !s.is_empty()) {
        format!("{stem}y")
    } else if let Some(stem) = name.strip_suffix("sses").filter(|s| !s.is_empty()) {
        format!("{stem}ss")
    } else if let Some(stem) = name.strip_suffix('s').filter(|s| {
        s.len() > 1 && !name.ends_with("ss") && !name.ends_with("us") && !name.ends_with("is")
    }) {
        stem.to_string()
    } else {
        name.to_string()
    }
}

/// 新しい表(主キー `id` の 1 列)を足す。つけた名前を返す。
pub fn add_table(d: &mut Design, base: &str) -> String {
    let name = d.unused_table_name(base);
    let mut t = Table::new(name.clone());
    t.logical = "新しい表".into();
    t.columns.push(Column {
        primary: true,
        nullable: false,
        auto: true,
        logical: "ID".into(),
        ..Column::new("id", DataType::Int)
    });
    d.tables.push(t);
    name
}

fn has_fk(t: &Table, columns: &[String], ref_table: &str) -> bool {
    t.foreign_keys
        .iter()
        .any(|f| f.columns == columns && f.ref_table == ref_table)
}

/// `child` が `parent` を参照する外部キーをつくる(子の列がなければ、親の主キーに合わせて足す)。
/// 返すのは、(子の表, 子の列, 親の表, 親の列)。
pub fn connect(
    d: &mut Design,
    child: &Pick,
    parent: &Pick,
) -> Result<(String, String, String, String), String> {
    // 主キーの列から、主キーでない列へ引いたときは、向きを入れ替える(外部キーは子が持つ)
    let is_pk = |d: &Design, p: &Pick| {
        p.column
            .as_ref()
            .and_then(|c| d.table(&p.table)?.column(c))
            .is_some_and(|c| c.primary)
    };
    let (child, parent) = if is_pk(d, child) && parent.column.is_some() && !is_pk(d, parent) {
        (parent, child)
    } else {
        (child, parent)
    };

    let parent_table = d
        .table(&parent.table)
        .ok_or_else(|| format!("表 '{}' がありません", parent.table))?;
    let parent_column = match &parent.column {
        Some(c) => parent_table
            .column(c)
            .ok_or_else(|| format!("列 '{}.{c}' がありません", parent.table))?
            .clone(),
        None => {
            let pk = parent_table.primary_key();
            match pk.as_slice() {
                [one] => parent_table
                    .column(one)
                    .cloned()
                    .unwrap_or_else(|| Column::new(one, DataType::Int)),
                [] => return Err(format!("表 '{}' に主キーがありません", parent.table)),
                _ => {
                    return Err(format!(
                        "表 '{}' の主キーは複数列です。列を選んでください",
                        parent.table
                    ));
                }
            }
        }
    };
    let child_index = d
        .table_index(&child.table)
        .ok_or_else(|| format!("表 '{}' がありません", child.table))?;
    let child_column = match &child.column {
        Some(c) => {
            if d.tables[child_index].column(c).is_none() {
                return Err(format!("列 '{}.{c}' がありません", child.table));
            }
            c.clone()
        }
        None => {
            let name = format!("{}_{}", singular(&parent.table), parent_column.name);
            let t = &mut d.tables[child_index];
            if t.column(&name).is_none() {
                t.columns.push(Column {
                    logical: parent_column.logical.clone(),
                    ..Column::new(name.clone(), parent_column.ty.clone())
                });
            }
            name
        }
    };
    let t = &mut d.tables[child_index];
    let columns = vec![child_column.clone()];
    if has_fk(t, &columns, &parent.table) {
        return Err("すでに同じ関係があります".into());
    }
    t.foreign_keys.push(ForeignKey {
        columns,
        ref_table: parent.table.clone(),
        ref_columns: vec![parent_column.name.clone()],
        ..Default::default()
    });
    Ok((
        child.table.clone(),
        child_column,
        parent.table.clone(),
        parent_column.name,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    fn shop() -> Design {
        parse("customers \"顧客\"\n  id int pk auto \"顧客ID\"\norders\n  id bigint pk auto\n  note text\nlines\n  a int pk\n  b int pk\n").unwrap()
    }

    #[test]
    fn singular_forms() {
        for (a, b) in [
            ("customers", "customer"),
            ("categories", "category"),
            ("addresses", "address"),
            ("status", "status"),
            ("s", "s"),
            ("data", "data"),
        ] {
            assert_eq!(singular(a), b, "{a}");
        }
    }

    #[test]
    fn selecting_two_tables_adds_the_foreign_key_column_and_the_relation() {
        let mut d = shop();
        let r = connect(&mut d, &Pick::table("orders"), &Pick::table("customers")).unwrap();
        assert_eq!(
            r,
            (
                "orders".into(),
                "customer_id".into(),
                "customers".into(),
                "id".into()
            )
        );
        let o = d.table("orders").unwrap();
        let c = o.column("customer_id").unwrap();
        assert_eq!(
            (c.ty.to_string().as_str(), c.logical.as_str()),
            ("int", "顧客ID")
        );
        assert!(c.nullable, "optional until the user says otherwise");
        assert_eq!(o.foreign_keys[0].ref_columns, ["id"]);
        // もう一度は、同じ関係になるので断る(列は増えない)
        let again = connect(&mut d, &Pick::table("orders"), &Pick::table("customers"));
        assert!(again.unwrap_err().contains("すでに"));
        assert_eq!(d.table("orders").unwrap().columns.len(), 3);
    }

    #[test]
    fn selecting_columns_uses_them_as_they_are() {
        let mut d = shop();
        let r = connect(
            &mut d,
            &Pick::column("orders", "note"),
            &Pick::column("customers", "id"),
        )
        .unwrap();
        assert_eq!(r.1, "note");
        assert_eq!(
            d.table("orders").unwrap().columns.len(),
            2,
            "no column is added"
        );
    }

    #[test]
    fn dragging_from_a_key_to_a_plain_column_swaps_the_direction() {
        let mut d = shop();
        let r = connect(
            &mut d,
            &Pick::column("customers", "id"),
            &Pick::column("orders", "note"),
        )
        .unwrap();
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.2.as_str()),
            ("orders", "note", "customers")
        );
    }

    #[test]
    fn mixed_picks_and_failures() {
        let mut d = shop();
        assert!(
            connect(&mut d, &Pick::table("orders"), &Pick::table("lines"))
                .unwrap_err()
                .contains("複数列")
        );
        d.tables.push(Table::new("empty"));
        assert!(
            connect(&mut d, &Pick::table("orders"), &Pick::table("empty"))
                .unwrap_err()
                .contains("主キーがありません")
        );
        assert!(connect(&mut d, &Pick::table("nope"), &Pick::table("customers")).is_err());
        assert!(
            connect(
                &mut d,
                &Pick::column("orders", "zz"),
                &Pick::table("customers")
            )
            .is_err()
        );
        // 列 → 表
        let r = connect(
            &mut d,
            &Pick::column("orders", "note"),
            &Pick::table("customers"),
        )
        .unwrap();
        assert_eq!(r.3, "id");
    }

    #[test]
    fn a_self_reference_is_allowed() {
        let mut d = shop();
        let r = connect(&mut d, &Pick::table("customers"), &Pick::table("customers")).unwrap();
        assert_eq!((r.0.as_str(), r.1.as_str()), ("customers", "customer_id"));
    }

    #[test]
    fn new_tables_get_unused_names() {
        let mut d = shop();
        assert_eq!(add_table(&mut d, "table"), "table");
        assert_eq!(add_table(&mut d, "table"), "table_2");
        let t = d.table("table_2").unwrap();
        assert_eq!(t.primary_key(), ["id"]);
    }
}
