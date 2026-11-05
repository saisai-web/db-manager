use super::*;
use crate::model::RefAction;
use crate::types::DataType;

const SAMPLE: &str = r#"
# コメント
@title ECサイト
@description "注文を扱う\n小さな例"
@author 佐伯
@version 1.0

customers "顧客" "会員の基本情報" @40,20
  id     int  pk auto   "顧客ID"
  name   varchar(100) not null "氏名" "フルネーム"
  email  varchar(255) unique
  status char(1) not null default 'A'
  created datetime default current_timestamp

orders "注文"
  id           bigint pk auto
  customer_id  int not null -> customers.id on delete cascade on update restrict "顧客"
  coupon_id    int -> coupons
  note         text default `a b`
  index (customer_id)
  unique uq_orders_note (note)

order_items
  order_id int pk
  line_no  int pk
  sku      varchar(20) not null
  fk fk_items_order (order_id) -> orders(id) on delete set null
  index ix_items_sku (sku) unique
"#;

#[test]
fn parses_a_whole_design() {
    let d = parse(SAMPLE).unwrap();
    assert_eq!(
        (d.title.as_str(), d.author.as_str(), d.version.as_str()),
        ("ECサイト", "佐伯", "1.0")
    );
    assert_eq!(d.description, "注文を扱う\n小さな例");
    assert_eq!(d.tables.len(), 3);

    let c = &d.tables[0];
    assert_eq!(
        (c.logical.as_str(), c.comment.as_str()),
        ("顧客", "会員の基本情報")
    );
    assert_eq!(c.pos, Some((40.0, 20.0)));
    let id = c.column("id").unwrap();
    assert!(id.primary && id.auto && !id.nullable);
    assert_eq!(id.logical, "顧客ID");
    let name = c.column("name").unwrap();
    assert_eq!(name.ty, DataType::Varchar(Some(100)));
    assert!(!name.nullable);
    assert_eq!(
        (name.logical.as_str(), name.comment.as_str()),
        ("氏名", "フルネーム")
    );
    assert!(c.column("email").unwrap().unique);
    assert_eq!(c.column("status").unwrap().default.as_deref(), Some("'A'"));
    assert_eq!(
        c.column("created").unwrap().default.as_deref(),
        Some("current_timestamp")
    );

    let o = &d.tables[1];
    assert_eq!(o.pos, None);
    assert_eq!(o.foreign_keys.len(), 2);
    let fk = &o.foreign_keys[0];
    assert_eq!(fk.columns, ["customer_id"]);
    assert_eq!(
        (fk.ref_table.as_str(), fk.ref_columns.as_slice()),
        ("customers", &["id".to_string()][..])
    );
    assert_eq!(
        (fk.on_delete, fk.on_update),
        (RefAction::Cascade, RefAction::Restrict)
    );
    assert!(
        o.foreign_keys[1].ref_columns.is_empty(),
        "-> table means its primary key"
    );
    assert_eq!(o.column("note").unwrap().default.as_deref(), Some("a b"));
    assert_eq!(o.indexes[0].columns, ["customer_id"]);
    assert_eq!(o.uniques[0].name, "uq_orders_note");

    let i = &d.tables[2];
    assert_eq!(i.primary_key(), ["order_id", "line_no"]);
    assert_eq!(i.foreign_keys[0].name, "fk_items_order");
    assert_eq!(i.foreign_keys[0].on_delete, RefAction::SetNull);
    assert!(i.indexes[0].unique);
}

#[test]
fn print_then_parse_gives_the_same_design() {
    let d = parse(SAMPLE).unwrap();
    let text = print(&d);
    let again = parse(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert_eq!(again, d, "{text}");
    assert_eq!(print(&again), text, "printing is stable");
}

#[test]
fn the_printed_text_is_compact_and_readable() {
    let text = print(&parse("t\n  id int pk auto\n  name varchar(10) !\n").unwrap());
    assert_eq!(
        text,
        "t\n  id   int         pk auto\n  name varchar(10) not null\n"
    );
}

#[test]
fn pk_and_primary_key_spellings_and_table_level_pk() {
    let d = parse("t\n  a int primary key\n  b int\n  c int\n  pk (b, c)\n").unwrap();
    assert_eq!(d.tables[0].primary_key(), ["a", "b", "c"]);
    assert!(d.tables[0].columns.iter().all(|c| !c.nullable));
}

#[test]
fn columns_named_like_keywords_are_still_columns() {
    let d = parse("t\n  index int\n  unique int\n  pk int\n  fk int\n").unwrap();
    assert_eq!(d.tables[0].columns.len(), 4);
    assert!(d.tables[0].indexes.is_empty());
}

#[test]
fn two_foreign_keys_on_one_column_survive_a_round_trip() {
    let src = "a\n  id int pk\nb\n  id int pk\nc\n  x int -> a.id\n  fk (x) -> b(id)\n";
    let d = parse(src).unwrap();
    assert_eq!(d.tables[2].foreign_keys.len(), 2);
    assert_eq!(parse(&print(&d)).unwrap(), d);
}

#[test]
fn errors_carry_line_numbers_and_all_are_reported() {
    let errs = parse("t\n  id\n  b int bogus\n  a int\n  a int\n").unwrap_err();
    let lines: Vec<_> = errs.iter().map(|e| e.line).collect();
    assert_eq!(lines, [2, 3, 5], "{errs:?}");
    assert!(errs[0].message.contains("型がありません"));
    assert!(errs[1].message.contains("bogus"));
    assert!(errs[2].message.contains("重複"));
    assert_eq!(errs[0].to_string(), format!("2 行目: {}", errs[0].message));
}

#[test]
fn structural_mistakes_are_rejected() {
    for (src, needle) in [
        ("  id int\n", "表の行の下"),
        ("@oops x\n", "不明な設定"),
        ("t\n  id int -> \n", "参照先"),
        ("t\n  id int -> a(x, y)\n", "1 列"),
        ("t\n  pk (zzz)\n", "zzz"),
        ("t\n  id int \"a\" \"b\" \"c\"\n", "2 つまで"),
        ("t\nt\n", "重複"),
        ("t \"a\n", "閉じられていません"),
        ("t.x\n", "使えません"),
        ("t\n  fk (a) b\n", "->"),
        ("t\n  a int on delete bogus\n", "不明な指定"),
        ("t\n  a int -> p on delete bogus\n", "動作は"),
        ("t @12\n", "位置"),
    ] {
        let errs = parse(src).expect_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains(needle)),
            "{src:?} -> {errs:?} (wanted {needle})"
        );
    }
}

#[test]
fn an_empty_text_is_an_empty_design() {
    assert_eq!(parse("").unwrap(), Design::default());
    assert_eq!(
        parse("\n# only comments\n// here\n").unwrap(),
        Design::default()
    );
    assert_eq!(print(&Design::default()), "");
}

#[test]
fn quotes_and_newlines_in_comments_round_trip() {
    let mut d = parse("t\n  a int\n").unwrap();
    d.tables[0].columns[0].comment = "say \"hi\"\n\\ ok".into();
    d.tables[0].columns[0].default = Some("now() + 1".into());
    assert_eq!(parse(&print(&d)).unwrap(), d);
}

use crate::model::Design;

#[test]
fn a_table_scale_round_trips_with_or_without_a_position() {
    let d = parse("a @10,20,1.5\n  id int pk\nb @auto,0.75\n  id int pk\nc @5,5\n  id int pk\n")
        .unwrap();
    assert_eq!(
        (d.tables[0].pos, d.tables[0].scale),
        (Some((10.0, 20.0)), Some(1.5))
    );
    assert_eq!((d.tables[1].pos, d.tables[1].scale), (None, Some(0.75)));
    assert_eq!(
        (d.tables[2].pos, d.tables[2].scale),
        (Some((5.0, 5.0)), None)
    );
    assert_eq!(parse(&print(&d)).unwrap(), d);
    assert!(parse("a @1,2,x\n  id int\n").is_err());
    assert!(parse("a @1\n  id int\n").is_err());
}
