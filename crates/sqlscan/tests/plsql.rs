use sqlscan::{Rules, StatementKind, split};

fn oracle() -> Rules {
    Rules {
        q_quote: true,
        plsql: true,
        ..Rules::generic()
    }
}

fn texts(sql: &str) -> Vec<String> {
    split(sql, &oracle())
        .iter()
        .map(|s| s.text(sql).to_string())
        .collect()
}

#[test]
fn a_block_is_one_statement_and_keeps_its_final_semicolon() {
    let sql = "select 1 from dual;\nBEGIN\n  NULL;\n  IF 1 = 1 THEN NULL; END IF;\nEND;\nselect 2 from dual";
    assert_eq!(
        texts(sql),
        [
            "select 1 from dual",
            "BEGIN\n  NULL;\n  IF 1 = 1 THEN NULL; END IF;\nEND;",
            "select 2 from dual"
        ]
    );
}

#[test]
fn a_block_is_never_read_only_even_though_it_starts_with_begin() {
    let sql = "BEGIN DELETE FROM t; END;";
    let sts = split(sql, &oracle());
    assert_eq!(sts.len(), 1);
    assert_eq!(sts[0].kind, StatementKind::Other);
    assert!(!sts[0].is_read_only());
}

#[test]
fn stored_programs_and_packages() {
    let sql = "CREATE OR REPLACE PROCEDURE p IS\nBEGIN\n  NULL;\nEND p;\n/\nCREATE OR REPLACE PACKAGE BODY k AS\n  PROCEDURE a IS BEGIN NULL; END;\nEND k;\ncreate table t (a int);";
    let t = texts(sql);
    assert_eq!(t.len(), 3, "{t:?}");
    assert!(t[0].starts_with("CREATE OR REPLACE PROCEDURE") && t[0].ends_with("END p;"));
    assert!(t[1].starts_with("CREATE OR REPLACE PACKAGE BODY") && t[1].ends_with("END k;"));
    assert_eq!(t[2], "create table t (a int)");
}

#[test]
fn a_slash_alone_on_its_line_ends_any_statement() {
    let sql = "select 1 from dual\n/\nselect 2 from dual\n/\n";
    assert_eq!(texts(sql), ["select 1 from dual", "select 2 from dual"]);
    // 行の途中の `/` は割り算
    assert_eq!(texts("select 4 / 2 from dual"), ["select 4 / 2 from dual"]);
}

#[test]
fn q_quoted_strings_hide_semicolons_and_quotes() {
    let sql = "select q'[it's; fine]' from dual; select 2 from dual";
    assert_eq!(
        texts(sql),
        ["select q'[it's; fine]' from dual", "select 2 from dual"]
    );
}

#[test]
fn without_the_plsql_rule_a_block_is_split_at_semicolons() {
    let sql = "BEGIN NULL; END;";
    assert_eq!(split(sql, &Rules::generic()).len(), 2);
}

#[test]
fn an_unfinished_block_runs_to_the_end_of_the_input() {
    let sql = "BEGIN NULL; select 1 from dual;";
    assert_eq!(texts(sql).len(), 1);
}
