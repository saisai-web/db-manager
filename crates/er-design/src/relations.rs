//! 関係(外部キー)の編集。名前・削除時/更新時の動き・親が必須か・1 対 1 か、を子の表の列に反映する。

use crate::model::{Design, ForeignKey, RefAction, UniqueKey};

fn fk_at(d: &mut Design, table: usize, fk: usize) -> Result<&mut ForeignKey, String> {
    d.tables
        .get_mut(table)
        .and_then(|t| t.foreign_keys.get_mut(fk))
        .ok_or_else(|| "関係が見つかりません".to_string())
}

/// 制約名を決める(空なら自動)。
pub fn set_name(d: &mut Design, table: usize, fk: usize, name: &str) -> Result<(), String> {
    let name = name.trim();
    if !name.is_empty() && !crate::dsl::is_valid_name(name) {
        return Err(format!(
            "制約の名前 '{name}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
        ));
    }
    fk_at(d, table, fk)?.name = name.to_string();
    Ok(())
}

/// 親の行を消したとき(`on_delete`)・変えたとき(それ以外)の動きを決める。
pub fn set_action(
    d: &mut Design,
    table: usize,
    fk: usize,
    on_delete: bool,
    action: RefAction,
) -> Result<(), String> {
    let f = fk_at(d, table, fk)?;
    if on_delete {
        f.on_delete = action;
    } else {
        f.on_update = action;
    }
    Ok(())
}

/// 親が必須か(子の列を NOT NULL にするか)。主キーの列は、もともと NULL にできない。
pub fn set_required(d: &mut Design, table: usize, fk: usize, required: bool) -> Result<(), String> {
    let columns = fk_at(d, table, fk)?.columns.clone();
    let t = &mut d.tables[table];
    for name in &columns {
        let Some(c) = t.columns.iter_mut().find(|c| &c.name == name) else {
            continue;
        };
        if !required && c.primary {
            return Err(format!(
                "{name} は主キーなので、NULL にできません(親は必須のままです)"
            ));
        }
        c.nullable = !required;
    }
    Ok(())
}

/// 1 対 1 か(子の列を一意にするか)。主キーそのものなら、もともと 1 対 1。
pub fn set_one_to_one(d: &mut Design, table: usize, fk: usize, on: bool) -> Result<(), String> {
    let columns = fk_at(d, table, fk)?.columns.clone();
    let t = &mut d.tables[table];
    let same =
        |cols: &[String]| cols.len() == columns.len() && columns.iter().all(|c| cols.contains(c));
    if !on && same(&t.primary_key()) {
        return Err("この列は主キーなので、1 対 1 のままです".into());
    }
    if let [name] = columns.as_slice()
        && let Some(c) = t.columns.iter_mut().find(|c| &c.name == name)
    {
        c.unique = on;
    }
    t.uniques.retain(|u| on || !same(&u.columns));
    if on && columns.len() > 1 && !t.uniques.iter().any(|u| same(&u.columns)) {
        t.uniques.push(UniqueKey {
            name: String::new(),
            columns,
        });
    }
    Ok(())
}

/// 関係を消す(列はそのまま残る)。
pub fn remove(d: &mut Design, table: usize, fk: usize) -> Result<(), String> {
    fk_at(d, table, fk)?;
    d.tables[table].foreign_keys.remove(fk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    fn design() -> Design {
        parse(
            "customers\n  id int pk\norders\n  id int pk\n  customer_id int -> customers.id\n  note text\n",
        )
        .unwrap()
    }

    #[test]
    fn name_and_actions_are_set_and_checked() {
        let mut d = design();
        let o = d.table_index("orders").unwrap();
        set_name(&mut d, o, 0, " fk_o_c ").unwrap();
        set_action(&mut d, o, 0, true, RefAction::Cascade).unwrap();
        set_action(&mut d, o, 0, false, RefAction::Restrict).unwrap();
        let f = &d.tables[o].foreign_keys[0];
        assert_eq!(
            (f.name.as_str(), f.on_delete, f.on_update),
            ("fk_o_c", RefAction::Cascade, RefAction::Restrict)
        );
        assert!(set_name(&mut d, o, 0, "bad name").is_err());
        assert!(set_name(&mut d, o, 5, "x").is_err());
        set_name(&mut d, o, 0, "").unwrap();
        assert!(d.tables[o].foreign_keys[0].name.is_empty());
    }

    #[test]
    fn required_and_one_to_one_follow_the_child_columns() {
        let mut d = design();
        let o = d.table_index("orders").unwrap();
        let fk = d.tables[o].foreign_keys[0].clone();
        assert!(!d.tables[o].fk_required(&fk) && !d.tables[o].fk_is_one_to_one(&fk));
        set_required(&mut d, o, 0, true).unwrap();
        set_one_to_one(&mut d, o, 0, true).unwrap();
        assert!(d.tables[o].fk_required(&fk) && d.tables[o].fk_is_one_to_one(&fk));
        set_required(&mut d, o, 0, false).unwrap();
        set_one_to_one(&mut d, o, 0, false).unwrap();
        assert!(!d.tables[o].fk_required(&fk) && !d.tables[o].fk_is_one_to_one(&fk));
    }

    #[test]
    fn a_primary_key_column_stays_required_and_one_to_one() {
        let mut d = parse("a\n  id int pk\nb\n  a_id int pk -> a.id\n").unwrap();
        let b = d.table_index("b").unwrap();
        assert!(set_required(&mut d, b, 0, false).is_err());
        assert!(set_one_to_one(&mut d, b, 0, false).is_err());
    }

    #[test]
    fn a_composite_relation_becomes_one_to_one_through_a_unique_key() {
        let mut d = parse(
            "p\n  a int pk\n  b int pk\nc\n  id int pk\n  a int\n  b int\n  fk (a, b) -> p(a, b)\n",
        )
        .unwrap();
        let c = d.table_index("c").unwrap();
        assert_eq!(d.tables[c].foreign_keys.len(), 1);
        set_one_to_one(&mut d, c, 0, true).unwrap();
        assert_eq!(d.tables[c].uniques.len(), 1);
        set_one_to_one(&mut d, c, 0, true).unwrap();
        assert_eq!(d.tables[c].uniques.len(), 1, "no duplicate");
        set_one_to_one(&mut d, c, 0, false).unwrap();
        assert!(d.tables[c].uniques.is_empty());
    }

    #[test]
    fn removing_a_relation_keeps_the_columns() {
        let mut d = design();
        let o = d.table_index("orders").unwrap();
        remove(&mut d, o, 0).unwrap();
        assert!(d.tables[o].foreign_keys.is_empty() && d.tables[o].column("customer_id").is_some());
        assert!(remove(&mut d, o, 0).is_err());
    }
}
