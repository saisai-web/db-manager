use proptest::prelude::*;
use sqlscan::{Rules, Statement, StatementKind, split, statement_at};

fn pg(sql: &str) -> Vec<Statement> {
    split(sql, &Rules::postgres())
}

fn texts<'a>(sql: &'a str, sts: &[Statement]) -> Vec<&'a str> {
    sts.iter().map(|s| s.text(sql)).collect()
}

#[test]
fn splits_on_semicolons_and_trims() {
    let sql = "select 1;\n  select 2 ;\n\n-- tail\n";
    let sts = pg(sql);
    assert_eq!(texts(sql, &sts), ["select 1", "select 2"]);
}

#[test]
fn last_statement_needs_no_terminator() {
    let sql = "select 1; select 2";
    assert_eq!(texts(sql, &pg(sql)), ["select 1", "select 2"]);
}

#[test]
fn empty_statements_are_dropped() {
    let sql = ";; /* c */ ; -- x\n;";
    assert!(pg(sql).is_empty());
    assert!(pg("").is_empty());
    assert!(pg("   \n").is_empty());
}

#[test]
fn leading_comments_are_not_part_of_the_statement() {
    let sql = "-- head\n/* a */ select 1 -- trailing\n;";
    let sts = pg(sql);
    assert_eq!(texts(sql, &sts), ["select 1"]);
    assert_eq!(sts[0].keyword.as_deref(), Some("SELECT"));
}

#[test]
fn semicolons_in_literals_comments_and_identifiers_do_not_split() {
    let sql = r#"select 'a;b', "c;d", $$e;f$$, $t$g;$$h$t$ /* ; */ -- ;
; select 2"#;
    let sts = pg(sql);
    assert_eq!(sts.len(), 2);
    assert_eq!(sts[1].text(sql), "select 2");
}

#[test]
fn function_body_in_dollar_quotes_is_one_statement() {
    let sql = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql; select f()";
    let sts = pg(sql);
    assert_eq!(sts.len(), 2);
    assert_eq!(sts[0].kind, StatementKind::Create);
    assert_eq!(sts[1].kind, StatementKind::Select);
}

#[test]
fn semicolons_inside_parentheses_do_not_split() {
    let sql = "CREATE RULE r AS ON INSERT TO t DO ALSO (insert into a values (1); insert into b values (2)); select 1";
    assert_eq!(pg(sql).len(), 2);
}

#[test]
fn begin_atomic_function_body_is_one_statement() {
    let sql = "CREATE FUNCTION f(x int) RETURNS int LANGUAGE sql \
               BEGIN ATOMIC select case when x > 0 then 1 else 2 end; select 3; END; select 9";
    let sts = pg(sql);
    assert_eq!(sts.len(), 2, "{:?}", texts(sql, &sts));
    assert_eq!(sts[1].text(sql), "select 9");
}

#[test]
fn plain_begin_is_a_transaction_statement_not_atomic() {
    let sql = "begin; select 1; commit";
    let sts = pg(sql);
    assert_eq!(texts(sql, &sts), ["begin", "select 1", "commit"]);
    assert_eq!(sts[0].kind, StatementKind::Transaction);
}

#[test]
fn unbalanced_close_paren_does_not_break_splitting() {
    let sql = "select 1); select 2";
    assert_eq!(pg(sql).len(), 2);
}

#[test]
fn unclosed_paren_swallows_the_rest_as_one_statement() {
    // 実行時に DB が構文エラーを返す。分割側で勝手に補正しない。
    let sql = "select (1; select 2";
    assert_eq!(pg(sql).len(), 1);
}

#[test]
fn keyword_is_uppercased_and_classified() {
    let cases = [
        ("select 1", "SELECT", StatementKind::Select),
        (
            "  Insert into t values (1)",
            "INSERT",
            StatementKind::Insert,
        ),
        ("update t set a=1", "UPDATE", StatementKind::Update),
        ("delete from t", "DELETE", StatementKind::Delete),
        ("drop table t", "DROP", StatementKind::Drop),
        ("truncate t", "TRUNCATE", StatementKind::Truncate),
        ("set search_path = x", "SET", StatementKind::Session),
        ("explain select 1", "EXPLAIN", StatementKind::Inspect),
        ("vacuum", "VACUUM", StatementKind::Other),
    ];
    for (sql, kw, kind) in cases {
        let st = &pg(sql)[0];
        assert_eq!(st.keyword.as_deref(), Some(kw), "{sql}");
        assert_eq!(st.kind, kind, "{sql}");
    }
}

#[test]
fn with_statement_takes_the_body_keyword() {
    let sql = "WITH RECURSIVE x AS (select 1) DELETE FROM t USING x";
    let st = &pg(sql)[0];
    assert_eq!(st.keyword.as_deref(), Some("DELETE"));
    assert_eq!(st.kind, StatementKind::Delete);
    assert!(st.is_unfiltered_dml());
}

#[test]
fn second_keyword_is_recorded() {
    let st = &pg("DROP TABLE IF EXISTS t")[0];
    assert_eq!(st.second_keyword.as_deref(), Some("TABLE"));
    assert!(st.is_destructive_ddl());
}

#[test]
fn where_detection_ignores_subqueries() {
    let no_where = &pg("update t set a = (select b from u where u.id = 1)")[0];
    assert!(!no_where.has_where);
    assert!(no_where.is_unfiltered_dml());

    let with_where = &pg("update t set a = 1 where id = 1")[0];
    assert!(with_where.has_where);
    assert!(!with_where.is_unfiltered_dml());

    let delete_where = &pg("delete from t where id in (select 1)")[0];
    assert!(!delete_where.is_unfiltered_dml());
}

#[test]
fn where_inside_a_string_or_comment_does_not_count() {
    let st = &pg("delete from t /* where x */ -- where y\n")[0];
    assert!(st.is_unfiltered_dml());
    let st = &pg("update t set a = 'where'")[0];
    assert!(st.is_unfiltered_dml());
}

#[test]
fn read_only_classification() {
    let ro = [
        "select 1",
        "values (1)",
        "table t",
        "show search_path",
        "explain select 1",
        "begin",
        "commit",
        "with x as (select 1) select * from x",
    ];
    for sql in ro {
        assert!(pg(sql)[0].is_read_only(), "{sql} should be read-only");
    }
    let rw = [
        "insert into t values (1)",
        "update t set a = 1",
        "delete from t",
        "drop table t",
        "create table t (a int)",
        "set default_transaction_read_only = off",
        "begin read write",
        "start transaction read write",
        "explain analyze delete from t",
        "with d as (delete from t returning *) select * from d",
        "with i as (insert into t values (1) returning *) select * from i",
        "vacuum",
        "do $$ begin end $$",
        // SELECT でも、読み取り専用の設定を外せてしまう
        "select set_config('default_transaction_read_only', 'off', false)",
        "SELECT SET_CONFIG('a', 'b', true)",
    ];
    for sql in rw {
        assert!(!pg(sql)[0].is_read_only(), "{sql} must not be read-only");
    }
}

#[test]
fn password_bearing_statements_are_flagged() {
    let flagged = [
        "CREATE USER bob WITH PASSWORD 'x'",
        "alter role bob password 'x'",
        "CREATE ROLE r LOGIN",
        "create server s foreign data wrapper w options (password 'x')",
        "select dblink_connect('host=h user=u Password=secret')",
        "ALTER USER bob RENAME TO alice",
    ];
    for sql in flagged {
        assert!(pg(sql)[0].may_contain_password, "{sql}");
    }
    let clean = [
        "select 1",
        "create table users (id int)",
        "drop table roles",
    ];
    for sql in clean {
        assert!(!pg(sql)[0].may_contain_password, "{sql}");
    }
}

#[test]
fn statement_at_cursor() {
    let sql = "select 1;\n\nselect 22;";
    let sts = pg(sql);
    let at = |off: usize| statement_at(&sts, off).map(|s| s.text(sql));

    assert_eq!(at(0), Some("select 1"));
    assert_eq!(at(8), Some("select 1"));
    assert_eq!(at(9), Some("select 1"), "直後の ; の位置は直前の文");
    assert_eq!(at(10), Some("select 1"), "文の間は直前の文");
    assert_eq!(at(11), Some("select 22"));
    assert_eq!(at(sql.len()), Some("select 22"));
}

#[test]
fn statement_at_before_first_statement_and_when_empty() {
    let sql = "\n\n select 1";
    let sts = pg(sql);
    assert_eq!(statement_at(&sts, 0).map(|s| s.text(sql)), Some("select 1"));
    assert!(statement_at(&[], 0).is_none());
}

#[test]
fn ranges_are_valid_for_multibyte_text() {
    let sql = "select '日本語;' as 列; select 表 from 'x'";
    let sts = pg(sql);
    assert_eq!(
        texts(sql, &sts),
        ["select '日本語;' as 列", "select 表 from 'x'"]
    );
}

proptest! {
    #[test]
    fn never_panics_and_ranges_are_ordered_char_boundaries(sql in "\\PC{0,200}") {
        let sts = pg(&sql);
        let mut prev_end = 0;
        for st in &sts {
            prop_assert!(sql.is_char_boundary(st.range.start));
            prop_assert!(sql.is_char_boundary(st.range.end));
            prop_assert!(st.range.start < st.range.end);
            prop_assert!(st.range.start >= prev_end);
            prop_assert!(st.end_with_terminator >= st.range.end);
            prop_assert!(st.end_with_terminator <= sql.len());
            prev_end = st.end_with_terminator;
        }
    }

    #[test]
    fn sql_like_noise_never_panics(sql in "[a-zA-Z0-9 ;'\"$()/*\\-\\n\\\\E#\\[\\]`]{0,120}") {
        for rules in [Rules::postgres(), Rules::generic()] {
            let _ = split(&sql, &rules);
        }
    }

    /// `;` を含まない単純な文は、`;` で連結して分割するともとの文に戻る。
    #[test]
    fn simple_statements_round_trip(parts in prop::collection::vec("[a-z][a-z0-9 ,=]{0,20}", 1..8)) {
        let parts: Vec<String> = parts.iter().map(|p| p.trim().to_string()).collect();
        let sql = parts.join(";\n");
        let sts = pg(&sql);
        prop_assert_eq!(texts(&sql, &sts), parts.iter().map(String::as_str).collect::<Vec<_>>());
    }

    /// どの位置でも、文があればいずれかの文が返る。
    #[test]
    fn statement_at_total_when_nonempty(sql in "[a-z ;\\n]{0,60}", off in 0usize..80) {
        let sts = pg(&sql);
        let off = off.min(sql.len());
        prop_assert_eq!(statement_at(&sts, off).is_some(), !sts.is_empty());
    }
}
