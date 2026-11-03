//! 補完の文脈ごとの候補。

use super::*;

fn catalog() -> Catalog {
    let t = |schema: &str, name: &str, cols: &[(&str, &str)]| CatalogTable {
        schema: schema.into(),
        name: name.into(),
        columns: cols
            .iter()
            .map(|(c, t)| (c.to_string(), t.to_string()))
            .collect(),
    };
    Catalog {
        tables: vec![
            t(
                "public",
                "customers",
                &[("id", "bigint"), ("name", "text"), ("email", "text")],
            ),
            t(
                "public",
                "orders",
                &[
                    ("id", "bigint"),
                    ("customer_id", "bigint"),
                    ("total", "numeric"),
                ],
            ),
            t("audit", "log", &[("at", "timestamptz")]),
        ],
    }
}

/// `|` の位置にカーソルを置いて、候補のラベルを返す。
fn at(sql_with_cursor: &str) -> (Vec<String>, String) {
    let offset = sql_with_cursor.find('|').unwrap();
    let sql = sql_with_cursor.replace('|', "");
    let s = suggest(&sql, offset, &Rules::postgres(), &catalog());
    let replaced = sql[s.replace.clone()].to_string();
    (
        s.candidates.into_iter().map(|c| c.label).collect(),
        replaced,
    )
}

#[test]
fn after_from_and_join_tables_are_offered() {
    let (c, replaced) = at("select * from cus|");
    assert_eq!(c.first().map(String::as_str), Some("customers"));
    assert_eq!(replaced, "cus");
    let (c, _) = at("select * from customers c join |");
    assert!(c.contains(&"orders".to_string()) && c.contains(&"audit".to_string()));
    assert!(
        !c.iter().any(|x| x == "id"),
        "no columns in a table position"
    );
}

#[test]
fn after_a_comma_in_a_from_list_tables_are_offered() {
    let (c, _) = at("select * from customers, ord|");
    assert_eq!(c.first().map(String::as_str), Some("orders"));
}

#[test]
fn a_dot_after_an_alias_offers_that_tables_columns() {
    let (c, replaced) = at("select c.| from customers c join orders o on o.customer_id = c.id");
    assert_eq!(c, ["email", "id", "name"]);
    assert_eq!(replaced, "");
    let (c, replaced) = at("select o.tot| from customers c join orders o on 1 = 1");
    assert_eq!(c, ["total"]);
    assert_eq!(replaced, "tot");
    // AS つきの別名、スキーマつきの表名
    let (c, _) = at("select x.| from audit.log as x");
    assert_eq!(c, ["at"]);
}

#[test]
fn a_dot_after_a_table_name_or_schema() {
    let (c, _) = at("select customers.| from customers");
    assert_eq!(c, ["email", "id", "name"]);
    let (c, _) = at("select * from audit.|");
    assert_eq!(c, ["log"]);
}

#[test]
fn in_an_expression_columns_of_the_statements_tables_come_before_keywords() {
    let (c, _) = at("select name, na| from customers");
    assert_eq!(c.first().map(String::as_str), Some("name"), "{c:?}");
    let (c, _) = at("select * from customers where e|");
    assert_eq!(c.first().map(String::as_str), Some("email"));
    assert!(
        c.iter().any(|x| x == "end" || x == "exists" || x == "else"),
        "keywords follow: {c:?}"
    );
}

#[test]
fn keywords_follow_the_case_the_user_types() {
    let (c, _) = at("sel|");
    assert!(c.contains(&"select".to_string()), "{c:?}");
    let (c, _) = at("SEL|");
    assert!(c.contains(&"SELECT".to_string()), "{c:?}");
}

#[test]
fn an_already_complete_word_is_not_offered_again() {
    let (c, _) = at("select * from customers|");
    assert!(!c.contains(&"customers".to_string()), "{c:?}");
}

#[test]
fn completion_applies_to_the_statement_under_the_cursor() {
    // 前の文のテーブルは、この文の列の候補にならない
    let (c, _) = at("select * from orders;\nselect na| from customers");
    assert_eq!(c.first().map(String::as_str), Some("name"));
    // 文の終端の後は、新しい文
    let (c, _) = at("select 1;\nsel|");
    assert!(c.contains(&"select".to_string()));
}

#[test]
fn quoted_identifiers_and_multibyte_text_do_not_break_offsets() {
    let (c, replaced) = at("select '日本語', cu| from customers");
    assert_eq!(replaced, "cu");
    let _ = c;
    let (c, _) = at("select \"c\".| from customers \"c\"");
    assert_eq!(c, ["email", "id", "name"]);
}

#[test]
fn empty_input_and_unknown_tables_give_keywords_only() {
    let (c, replaced) = at("|");
    assert!(c.contains(&"SELECT".to_string()));
    assert_eq!(replaced, "");
    let (c, _) = at("select nope.| from nothing nope");
    assert!(c.is_empty());
}
