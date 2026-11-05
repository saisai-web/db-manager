//! 列の編集。名前の変更は、その列を使う外部キー・一意制約・索引にも追従する。

use crate::model::{Column, Design};
use crate::types::DataType;

fn table_at(d: &mut Design, table: usize) -> Result<&mut crate::model::Table, String> {
    d.tables
        .get_mut(table)
        .ok_or_else(|| "表が見つかりません".to_string())
}

/// 表の末尾に列を足す(`column`、`column_2`、…)。つけた名前を返す。
pub fn add_column(d: &mut Design, table: usize) -> Result<String, String> {
    let t = table_at(d, table)?;
    let name = (1..)
        .map(|n| {
            if n == 1 {
                "column".to_string()
            } else {
                format!("column_{n}")
            }
        })
        .find(|n| t.column(n).is_none())
        .unwrap_or_default();
    t.columns
        .push(Column::new(name.clone(), DataType::Varchar(Some(255))));
    Ok(name)
}

/// 列の名前を変える。同じ表の外部キー・一意制約・索引と、この列を参照する他の表の外部キーも追従する。
pub fn rename_column(d: &mut Design, table: usize, from: &str, to: &str) -> Result<(), String> {
    let to = to.trim();
    if to.is_empty() {
        return Err("列の名前が空です".into());
    }
    if to == from {
        return Ok(());
    }
    let table_name = {
        let t = table_at(d, table)?;
        if t.column(to).is_some() {
            return Err(format!("列 {to} は、すでにあります"));
        }
        let c = t
            .columns
            .iter_mut()
            .find(|c| c.name == from)
            .ok_or_else(|| format!("列 {from} が見つかりません"))?;
        c.name = to.to_string();
        let swap = |cols: &mut Vec<String>| {
            cols.iter_mut()
                .filter(|c| c.as_str() == from)
                .for_each(|c| *c = to.to_string());
        };
        t.uniques.iter_mut().for_each(|u| swap(&mut u.columns));
        t.indexes.iter_mut().for_each(|i| swap(&mut i.columns));
        for f in &mut t.foreign_keys {
            swap(&mut f.columns);
            if f.ref_table == t.name {
                swap(&mut f.ref_columns);
            }
        }
        t.name.clone()
    };
    for other in d.tables.iter_mut().filter(|o| o.name != table_name) {
        for f in other
            .foreign_keys
            .iter_mut()
            .filter(|f| f.ref_table == table_name)
        {
            f.ref_columns
                .iter_mut()
                .filter(|c| c.as_str() == from)
                .for_each(|c| *c = to.to_string());
        }
    }
    Ok(())
}

/// 列を消す。その列を使う外部キー・一意制約・索引と、この列を参照する他の表の外部キーも消す。
pub fn remove_column(d: &mut Design, table: usize, column: &str) -> Result<(), String> {
    let table_name = {
        let t = table_at(d, table)?;
        let before = t.columns.len();
        t.columns.retain(|c| c.name != column);
        if t.columns.len() == before {
            return Err(format!("列 {column} が見つかりません"));
        }
        let uses = |cols: &[String]| cols.iter().any(|c| c == column);
        t.uniques.retain(|u| !uses(&u.columns));
        t.indexes.retain(|i| !uses(&i.columns));
        t.foreign_keys.retain(|f| !uses(&f.columns));
        t.name.clone()
    };
    for other in &mut d.tables {
        other
            .foreign_keys
            .retain(|f| f.ref_table != table_name || !f.ref_columns.iter().any(|c| c == column));
    }
    Ok(())
}

/// 列を `delta` だけ上(負)・下(正)へ動かす。端なら何もしない。
pub fn move_column(d: &mut Design, table: usize, index: usize, delta: isize) -> Result<(), String> {
    let t = table_at(d, table)?;
    let to = index
        .checked_add_signed(delta)
        .filter(|&i| i < t.columns.len());
    if let (Some(to), true) = (to, index < t.columns.len()) {
        t.columns.swap(index, to);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    fn design() -> Design {
        parse(
            "orgs\n  id int pk\n  code varchar(10) unique\norders\n  id int pk\n  org_id int -> orgs.id\n  org_code varchar(10) -> orgs.code\n  unique (org_id, id)\n  index (org_id)\n",
        )
        .unwrap()
    }

    #[test]
    fn adding_names_columns_without_clashes() {
        let mut d = design();
        assert_eq!(add_column(&mut d, 0).unwrap(), "column");
        assert_eq!(add_column(&mut d, 0).unwrap(), "column_2");
        assert_eq!(d.tables[0].columns.len(), 4);
        assert!(add_column(&mut d, 9).is_err());
    }

    #[test]
    fn renaming_follows_every_use_of_the_column() {
        let mut d = design();
        rename_column(&mut d, 0, "id", "org_pk").unwrap();
        assert_eq!(d.tables[1].foreign_keys[0].ref_columns, ["org_pk"]);
        rename_column(&mut d, 1, "org_id", "owner").unwrap();
        let o = &d.tables[1];
        assert_eq!(o.foreign_keys[0].columns, ["owner"]);
        assert_eq!(o.uniques[0].columns, ["owner", "id"]);
        assert_eq!(o.indexes[0].columns, ["owner"]);
        assert!(o.column("owner").is_some() && o.column("org_id").is_none());
    }

    #[test]
    fn a_bad_rename_changes_nothing() {
        let mut d = design();
        let before = d.clone();
        assert!(rename_column(&mut d, 0, "id", "code").is_err());
        assert!(rename_column(&mut d, 0, "id", "  ").is_err());
        assert!(rename_column(&mut d, 0, "nope", "x").is_err());
        assert_eq!(d, before);
        assert!(rename_column(&mut d, 0, "id", "id").is_ok());
    }

    #[test]
    fn removing_drops_what_depended_on_the_column() {
        let mut d = design();
        remove_column(&mut d, 1, "org_id").unwrap();
        let o = &d.tables[1];
        assert!(o.uniques.is_empty() && o.indexes.is_empty());
        assert_eq!(o.foreign_keys.len(), 1);
        // 親の列を消すと、それを参照していた外部キーも消える
        remove_column(&mut d, 0, "code").unwrap();
        assert!(d.tables[1].foreign_keys.is_empty());
        assert!(remove_column(&mut d, 0, "code").is_err());
    }

    #[test]
    fn moving_swaps_neighbours_and_stops_at_the_ends() {
        let mut d = design();
        move_column(&mut d, 1, 2, -1).unwrap();
        assert_eq!(d.tables[1].columns[1].name, "org_code");
        move_column(&mut d, 1, 0, -1).unwrap();
        move_column(&mut d, 1, 2, 1).unwrap();
        move_column(&mut d, 1, 9, 1).unwrap();
        assert_eq!(d.tables[1].columns[0].name, "id");
    }
}
