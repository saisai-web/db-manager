//! 識別子と文字列の引用符。

use super::Target;

/// どれかの DB で予約語になる語(小文字)。引用符で囲んで、どの DB でも通るようにする。
const RESERVED: &[&str] = &[
    "all",
    "alter",
    "and",
    "any",
    "as",
    "asc",
    "between",
    "by",
    "case",
    "check",
    "column",
    "comment",
    "constraint",
    "create",
    "cross",
    "current",
    "database",
    "date",
    "default",
    "delete",
    "desc",
    "describe",
    "distinct",
    "drop",
    "else",
    "empty",
    "end",
    "exists",
    "false",
    "file",
    "for",
    "foreign",
    "from",
    "full",
    "function",
    "grant",
    "group",
    "groups",
    "having",
    "identity",
    "in",
    "index",
    "inner",
    "insert",
    "int",
    "integer",
    "into",
    "is",
    "join",
    "key",
    "kill",
    "lateral",
    "left",
    "level",
    "like",
    "limit",
    "load",
    "lock",
    "match",
    "modify",
    "not",
    "null",
    "number",
    "of",
    "offset",
    "on",
    "option",
    "or",
    "order",
    "outer",
    "over",
    "percent",
    "primary",
    "public",
    "range",
    "rank",
    "read",
    "references",
    "rename",
    "replace",
    "return",
    "right",
    "row",
    "rows",
    "schema",
    "select",
    "session",
    "set",
    "show",
    "size",
    "system",
    "table",
    "then",
    "to",
    "trigger",
    "true",
    "uid",
    "union",
    "unique",
    "update",
    "use",
    "user",
    "values",
    "view",
    "when",
    "where",
    "window",
    "with",
];

fn is_plain(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// 識別子。普通の名前ならそのまま、予約語や日本語などは引用符で囲む。
/// PostgreSQL は、大文字を含む名前を大文字小文字のまま残すため囲む。
pub fn ident(target: Target, name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let needs = !is_plain(name)
        || RESERVED.contains(&lower.as_str())
        || (target == Target::Postgres && name != lower);
    if !needs {
        return name.to_string();
    }
    match target {
        Target::MySql => format!("`{}`", name.replace('`', "``")),
        Target::SqlServer => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// `schema.table`(スキーマが空なら表の名前だけ)。
pub fn qualified(target: Target, schema: &str, name: &str) -> String {
    if schema.is_empty() {
        ident(target, name)
    } else {
        format!("{}.{}", ident(target, schema), ident(target, name))
    }
}

pub fn ident_list(target: Target, names: &[String]) -> String {
    names
        .iter()
        .map(|n| ident(target, n))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 文字列リテラル。SQL Server は日本語のために `N'...'`。MySQL は `\` もエスケープする。
pub fn literal(target: Target, s: &str) -> String {
    let body = if target == Target::MySql {
        s.replace('\\', "\\\\").replace('\'', "''")
    } else {
        s.replace('\'', "''")
    };
    let n = if target == Target::SqlServer { "N" } else { "" };
    format!("{n}'{body}'")
}

/// コメントに使う文: 論理名と備考を 1 つにする。
pub fn describe(logical: &str, comment: &str) -> String {
    match (logical.is_empty(), comment.is_empty()) {
        (false, false) => format!("{logical}: {comment}"),
        (false, true) => logical.to_string(),
        (true, _) => comment.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_stay_bare_and_others_are_quoted_in_each_style() {
        assert_eq!(ident(Target::Postgres, "orders"), "orders");
        assert_eq!(ident(Target::Postgres, "order"), "\"order\"");
        assert_eq!(
            ident(Target::Postgres, "Orders"),
            "\"Orders\"",
            "keeps its case"
        );
        assert_eq!(
            ident(Target::Oracle, "Orders"),
            "Orders",
            "folds to upper case"
        );
        assert_eq!(ident(Target::MySql, "user"), "`user`");
        assert_eq!(ident(Target::SqlServer, "顧客"), "[顧客]");
        assert_eq!(ident(Target::Sqlite, "a\"b"), "\"a\"\"b\"");
        assert_eq!(ident(Target::SqlServer, "a]b"), "[a]]b]");
        assert_eq!(ident(Target::MySql, "a`b"), "`a``b`");
    }

    #[test]
    fn qualified_names_and_lists() {
        assert_eq!(qualified(Target::Postgres, "app", "user"), "app.\"user\"");
        assert_eq!(qualified(Target::Postgres, "", "t"), "t");
        assert_eq!(
            ident_list(Target::MySql, &["a".into(), "group".into()]),
            "a, `group`"
        );
    }

    #[test]
    fn literals_escape_quotes_and_backslashes_where_needed() {
        assert_eq!(literal(Target::Postgres, "it's"), "'it''s'");
        assert_eq!(literal(Target::MySql, "a\\b'"), "'a\\\\b'''");
        assert_eq!(literal(Target::SqlServer, "日本"), "N'日本'");
    }

    #[test]
    fn descriptions_join_logical_name_and_comment() {
        assert_eq!(describe("顧客", "会員"), "顧客: 会員");
        assert_eq!(describe("顧客", ""), "顧客");
        assert_eq!(describe("", "メモ"), "メモ");
        assert_eq!(describe("", ""), "");
    }
}
