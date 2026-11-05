//! 設計の点検。DDL にする前・設計書に載せる前に、直したほうがよい点を挙げる。

use std::collections::HashSet;

use crate::model::{Design, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// DDL が正しく作れない
    Error,
    /// 作れるが、見直したほうがよい
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// 該当する表(設計全体の問題なら空)
    pub table: String,
    pub message: String,
}

fn error(table: &str, message: String) -> Issue {
    Issue {
        severity: Severity::Error,
        table: table.into(),
        message,
    }
}

fn warning(table: &str, message: String) -> Issue {
    Issue {
        severity: Severity::Warning,
        table: table.into(),
        message,
    }
}

pub fn validate(d: &Design) -> Vec<Issue> {
    let mut issues = Vec::new();
    for t in &d.tables {
        check_table(d, t, &mut issues);
    }
    issues.sort_by_key(|i| i.severity == Severity::Warning);
    issues
}

fn missing_columns(t: &Table, cols: &[String]) -> Vec<String> {
    cols.iter()
        .filter(|c| t.column(c).is_none())
        .cloned()
        .collect()
}

fn check_table(d: &Design, t: &Table, out: &mut Vec<Issue>) {
    if t.columns.is_empty() {
        out.push(error(&t.name, "列がありません".into()));
    }
    if !t.columns.is_empty() && t.primary_key().is_empty() {
        out.push(warning(&t.name, "主キーがありません".into()));
    }
    for (what, cols) in t
        .uniques
        .iter()
        .map(|u| ("一意制約", &u.columns))
        .chain(t.indexes.iter().map(|i| ("索引", &i.columns)))
    {
        let missing = missing_columns(t, cols);
        if !missing.is_empty() {
            out.push(error(
                &t.name,
                format!("{what}の列 {} がありません", missing.join(", ")),
            ));
        }
    }
    let mut names = HashSet::new();
    for fk in &t.foreign_keys {
        check_foreign_key(d, t, fk, out);
        if !names.insert(t.fk_name(fk)) {
            out.push(error(
                &t.name,
                format!("外部キー名 '{}' が重複しています", t.fk_name(fk)),
            ));
        }
    }
}

fn check_foreign_key(d: &Design, t: &Table, fk: &crate::model::ForeignKey, out: &mut Vec<Issue>) {
    let label = format!("外部キー {}", t.fk_name(fk));
    let missing = missing_columns(t, &fk.columns);
    if !missing.is_empty() {
        out.push(error(
            &t.name,
            format!("{label}: 列 {} がありません", missing.join(", ")),
        ));
        return;
    }
    let Some(parent) = d.table(&fk.ref_table) else {
        out.push(error(
            &t.name,
            format!("{label}: 参照先の表 '{}' がありません", fk.ref_table),
        ));
        return;
    };
    let ref_cols = d.ref_columns(fk);
    if ref_cols.is_empty() {
        out.push(error(
            &t.name,
            format!(
                "{label}: 参照先 '{}' に主キーがなく、列を指定できません",
                parent.name
            ),
        ));
        return;
    }
    let missing = missing_columns(parent, &ref_cols);
    if !missing.is_empty() {
        out.push(error(
            &t.name,
            format!(
                "{label}: 参照先 '{}' に列 {} がありません",
                parent.name,
                missing.join(", ")
            ),
        ));
        return;
    }
    if ref_cols.len() != fk.columns.len() {
        out.push(error(
            &t.name,
            format!(
                "{label}: 列の数が合いません({} 列 → {} 列)",
                fk.columns.len(),
                ref_cols.len()
            ),
        ));
        return;
    }
    for (c, p) in fk.columns.iter().zip(&ref_cols) {
        let (child, parent_col) = (t.column(c), parent.column(p));
        if let (Some(a), Some(b)) = (child, parent_col)
            && !a.ty.compatible_with(&b.ty)
        {
            out.push(warning(
                &t.name,
                format!(
                    "{label}: {c}({})と {}.{p}({})の型が合いません",
                    a.ty, parent.name, b.ty
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    fn issues(src: &str) -> Vec<(Severity, String)> {
        validate(&parse(src).unwrap())
            .into_iter()
            .map(|i| (i.severity, i.message))
            .collect()
    }

    #[test]
    fn a_clean_design_has_no_issues() {
        assert!(issues("a\n  id int pk\nb\n  id int pk\n  a_id int -> a.id\n").is_empty());
    }

    #[test]
    fn missing_keys_columns_and_tables_are_reported() {
        let got = issues("a\n  x int\nb\n  id int pk\n  y int -> nope.id\n  index (zz)\nc\n");
        let msgs: Vec<_> = got.iter().map(|(_, m)| m.as_str()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("参照先の表 'nope'")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("索引の列 zz")), "{msgs:?}");
        assert!(msgs.contains(&"列がありません"), "{msgs:?}");
        assert!(msgs.contains(&"主キーがありません"), "{msgs:?}");
        assert!(
            got.first().is_some_and(|(s, _)| *s == Severity::Error),
            "errors come first"
        );
        assert_eq!(got.last().unwrap().0, Severity::Warning);
    }

    #[test]
    fn reference_shape_problems() {
        let got = issues(
            "p\n  a int pk\n  b int pk\nc\n  id int pk\n  x int -> p\n  y int -> p.zz\n  z uuid -> p.a\n",
        );
        let msgs: Vec<_> = got.iter().map(|(_, m)| m.as_str()).collect();
        assert!(
            msgs.iter().any(|m| m.contains("列の数が合いません")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("列 zz がありません")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("型が合いません")),
            "{msgs:?}"
        );
    }

    #[test]
    fn omitted_reference_columns_need_a_primary_key_on_the_parent() {
        let got = issues("p\n  a int\nc\n  id int pk\n  x int -> p\n");
        assert!(
            got.iter().any(|(_, m)| m.contains("主キーがなく")),
            "{got:?}"
        );
    }
}
