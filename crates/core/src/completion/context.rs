//! カーソル位置の文脈(テーブル名の位置か、`別名.` の後か、式の中か)と、文中の表と別名。

use sqlscan::{Token, TokenKind};

pub(super) fn upper(t: &Token, sql: &str) -> String {
    t.text(sql).to_ascii_uppercase()
}

/// どの種類の候補を出すか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Context {
    /// FROM / JOIN / INTO などの直後
    Table,
    /// `qualifier.` の後
    Qualified(String),
    /// それ以外(式の中)
    Expression,
}

pub(super) fn context(tokens: &[Token], sql: &str, prefix_start: usize) -> Context {
    let before: Vec<&Token> = tokens
        .iter()
        .filter(|t| !t.kind.is_trivia() && t.end <= prefix_start)
        .collect();
    let Some(last) = before.last() else {
        return Context::Expression;
    };
    if last.kind == TokenKind::Punct
        && last.text(sql) == "."
        && let Some(q) = before.len().checked_sub(2).and_then(|i| before.get(i))
        && matches!(q.kind, TokenKind::Word | TokenKind::QuotedIdent)
    {
        return Context::Qualified(unquote(q.text(sql)));
    }
    const TABLE_KEYWORDS: [&str; 8] = [
        "FROM", "JOIN", "INTO", "UPDATE", "TABLE", "TRUNCATE", "ANALYZE", "DESCRIBE",
    ];
    if last.kind == TokenKind::Word && TABLE_KEYWORDS.contains(&upper(last, sql).as_str()) {
        return Context::Table;
    }
    // FROM a, b の、カンマの後
    if last.kind == TokenKind::Punct && last.text(sql) == "," {
        let clause = before.iter().rev().find(|t| {
            t.kind == TokenKind::Word
                && [
                    "FROM", "SELECT", "WHERE", "SET", "GROUP", "ORDER", "VALUES", "ON",
                ]
                .contains(&upper(t, sql).as_str())
        });
        if clause.is_some_and(|t| upper(t, sql) == "FROM") {
            return Context::Table;
        }
    }
    Context::Expression
}

pub(super) fn unquote(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_string()
}

/// 文中の `FROM t [AS] a` / `JOIN s.t a` から、(別名 or 表名, 表名)の対応を取る。
pub(super) fn tables_in_statement(tokens: &[Token], sql: &str) -> Vec<(String, String)> {
    let sig: Vec<&Token> = tokens.iter().filter(|t| !t.kind.is_trivia()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < sig.len() {
        let t = sig[i];
        let is_intro = t.kind == TokenKind::Word
            && matches!(upper(t, sql).as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO")
            || t.kind == TokenKind::Punct && t.text(sql) == "," && in_from_list(&sig[..i], sql);
        i += 1;
        if !is_intro {
            continue;
        }
        let Some(name_tok) = sig
            .get(i)
            .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent))
        else {
            continue;
        };
        let mut table = unquote(name_tok.text(sql));
        i += 1;
        // schema.table
        if sig.get(i).is_some_and(|d| d.text(sql) == ".")
            && let Some(n) = sig
                .get(i + 1)
                .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent))
        {
            table = unquote(n.text(sql));
            i += 2;
        }
        let mut alias = None;
        if sig
            .get(i)
            .is_some_and(|t| t.kind == TokenKind::Word && upper(t, sql) == "AS")
        {
            i += 1;
        }
        let alias_token = sig.get(i).filter(|t| {
            t.kind == TokenKind::QuotedIdent
                || t.kind == TokenKind::Word && !is_reserved(&upper(t, sql))
        });
        if let Some(a) = alias_token {
            alias = Some(unquote(a.text(sql)));
            i += 1;
        }
        out.push((alias.unwrap_or_else(|| table.clone()), table));
    }
    out
}

pub(super) fn in_from_list(before: &[&Token], sql: &str) -> bool {
    before
        .iter()
        .rev()
        .find(|t| {
            t.kind == TokenKind::Word
                && matches!(
                    upper(t, sql).as_str(),
                    "FROM" | "SELECT" | "WHERE" | "SET" | "ON" | "VALUES" | "GROUP" | "ORDER"
                )
        })
        .is_some_and(|t| upper(t, sql) == "FROM")
}

pub(super) fn is_reserved(word: &str) -> bool {
    matches!(
        word,
        "WHERE"
            | "ON"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "CROSS"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "UNION"
            | "SET"
            | "USING"
            | "AND"
            | "OR"
            | "VALUES"
            | "RETURNING"
            | "SELECT"
    )
}
