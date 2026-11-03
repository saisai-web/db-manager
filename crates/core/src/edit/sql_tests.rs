//! SQL の生成。PostgreSQL の Dialect で、生成される文とバインド値を確かめる。

use driver_api::{ColumnInfo, TableRef};
use driver_postgres::PostgresDialect;

use super::*;

fn col(name: &str, ty: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_name: ty.into(),
        nullable: true,
        default: None,
        ordinal: 1,
    }
}

fn source() -> TableSource {
    TableSource {
        table: TableRef::new("public", "My Table"),
        columns: vec![
            col("id", "bigint"),
            col("price", "numeric(20,6)"),
            col("name", "text"),
            col("at", "timestamp with time zone"),
        ],
        key: vec!["id".into()],
    }
}

fn s(v: &str) -> Value {
    Some(v.to_string())
}

fn one(change: Change) -> PlannedEdit {
    plan(&source(), &[change], &PostgresDialect)
        .unwrap()
        .remove(0)
}

#[test]
fn update_binds_new_values_then_key_then_the_original_values() {
    let p = one(Change::Update {
        key: vec![("id".into(), s("42"))],
        set: vec![
            ("name".into(), s("it's"), s("old")),
            ("price".into(), None, s("1.50")),
        ],
    });
    assert_eq!(
        p.sql,
        "UPDATE \"public\".\"My Table\" SET \"name\" = ($1::text)::text, \"price\" = ($2::text)::numeric(20,6) \
         WHERE \"id\" = ($3::text)::bigint \
         AND \"name\" IS NOT DISTINCT FROM ($4::text)::text \
         AND \"price\" IS NOT DISTINCT FROM ($5::text)::numeric(20,6)"
    );
    assert_eq!(p.params, [s("it's"), None, s("42"), s("old"), s("1.50")]);
    assert_eq!(p.kind, ChangeKind::Update);
}

#[test]
fn the_preview_shows_values_as_literals_and_never_executes_them() {
    let p = one(Change::Update {
        key: vec![("id".into(), s("42"))],
        set: vec![
            ("name".into(), s("it's"), None),
            ("price".into(), s("9.99"), s("1.50")),
        ],
    });
    assert_eq!(
        p.preview,
        "UPDATE \"public\".\"My Table\" SET \"name\" = 'it''s', \"price\" = 9.99 \
         WHERE \"id\" = 42 AND \"name\" IS NOT DISTINCT FROM NULL AND \"price\" IS NOT DISTINCT FROM 1.50"
    );
    // 実行する SQL には、値そのものが入らない
    assert!(!p.sql.contains("it's") && !p.sql.contains("9.99"));
}

#[test]
fn values_that_look_like_sql_stay_values() {
    let evil = "x'); DROP TABLE t; --";
    let p = one(Change::Update {
        key: vec![("id".into(), s("1"))],
        set: vec![("name".into(), s(evil), s("a"))],
    });
    assert!(!p.sql.contains("DROP"));
    assert!(p.params.contains(&s(evil)));
    assert!(
        p.preview.contains("'x''); DROP TABLE t; --'"),
        "quoted in the preview: {}",
        p.preview
    );
}

#[test]
fn numbers_in_the_preview_are_bare_only_for_numeric_columns_and_valid_numbers() {
    let p = one(Change::Insert {
        values: vec![
            ("id".into(), s("7")),
            ("name".into(), s("7")),
            ("price".into(), s("1e5")),
        ],
    });
    // 文字列列の 7 は引用符つき。"1e5" は素の数値として書けないので引用符つき
    assert_eq!(
        p.preview,
        "INSERT INTO \"public\".\"My Table\" (\"id\", \"name\", \"price\") VALUES (7, '7', '1e5')"
    );
}

#[test]
fn delete_uses_only_the_key() {
    let p = one(Change::Delete {
        key: vec![("id".into(), s("5"))],
    });
    assert_eq!(
        p.sql,
        "DELETE FROM \"public\".\"My Table\" WHERE \"id\" = ($1::text)::bigint"
    );
    assert_eq!(p.params, [s("5")]);
}

#[test]
fn insert_lists_only_the_given_columns_and_may_be_all_defaults() {
    let p = one(Change::Insert {
        values: vec![
            ("name".into(), None),
            ("at".into(), s("2026-11-02 12:00:00+09")),
        ],
    });
    assert_eq!(
        p.sql,
        "INSERT INTO \"public\".\"My Table\" (\"name\", \"at\") VALUES (($1::text)::text, ($2::text)::timestamp with time zone)"
    );
    assert_eq!(p.params, [None, s("2026-11-02 12:00:00+09")]);
    let d = one(Change::Insert { values: vec![] });
    assert_eq!(d.sql, "INSERT INTO \"public\".\"My Table\" DEFAULT VALUES");
    assert!(d.params.is_empty());
}

#[test]
fn composite_keys_are_all_required() {
    let mut src = source();
    src.key = vec!["id".into(), "name".into()];
    let p = plan(
        &src,
        &[Change::Delete {
            key: vec![("id".into(), s("1")), ("name".into(), s("a"))],
        }],
        &PostgresDialect,
    )
    .unwrap();
    assert!(
        p[0].sql
            .ends_with("WHERE \"id\" = ($1::text)::bigint AND \"name\" = ($2::text)::text")
    );
}

#[test]
fn invalid_changes_are_rejected_before_any_sql_is_made() {
    let d = &PostgresDialect;
    assert_eq!(plan(&source(), &[], d), Err(EditError::NoChanges));
    assert_eq!(
        plan(
            &source(),
            &[Change::Delete {
                key: vec![("id".into(), None)]
            }],
            d
        ),
        Err(EditError::NullKey)
    );
    assert_eq!(
        plan(&source(), &[Change::Delete { key: vec![] }], d),
        Err(EditError::EmptyKey)
    );
    assert_eq!(
        plan(
            &source(),
            &[Change::Insert {
                values: vec![("nope".into(), s("x"))]
            }],
            d
        ),
        Err(EditError::UnknownColumn("nope".into()))
    );
    assert_eq!(
        plan(
            &source(),
            &[Change::Update {
                key: vec![("id".into(), s("1"))],
                set: vec![]
            }],
            d
        ),
        Err(EditError::NoChanges)
    );
}
