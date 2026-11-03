use driver_api::{Dialect, LexicalFeatures, TableRef};

#[derive(Debug, Default)]
pub struct SqliteDialect;

impl Dialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn quote_literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    fn lexical_features(&self) -> LexicalFeatures {
        // `"x"`、`` `x` ``、`[x]` のどれでも識別子を書ける
        LexicalFeatures {
            backtick_ident: true,
            bracket_ident: true,
            ..LexicalFeatures::default()
        }
    }

    /// 値は文字列で渡す。列の型親和性が、必要なら数値に変換する。
    fn bind_text(&self, n: usize, _type_name: &str) -> String {
        format!("?{n}")
    }

    fn null_safe_equals(&self, lhs: &str, rhs: &str) -> String {
        format!("{lhs} IS {rhs}")
    }

    fn select_table_sql(&self, table: &TableRef, limit: Option<u64>) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name(table));
        if let Some(n) = limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        sql
    }

    fn set_read_only_sql(&self, read_only: bool) -> Option<String> {
        Some(format!(
            "PRAGMA query_only = {}",
            if read_only { "ON" } else { "OFF" }
        ))
    }

    fn explain_sql(&self, sql: &str, _analyze: bool) -> Option<String> {
        Some(format!("EXPLAIN QUERY PLAN {sql}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_specific_sql() {
        let d = SqliteDialect;
        assert_eq!(d.bind_text(3, "text"), "?3");
        assert_eq!(d.null_safe_equals("\"a\"", "?1"), "\"a\" IS ?1");
        assert_eq!(d.set_read_only_sql(true).unwrap(), "PRAGMA query_only = ON");
        assert_eq!(d.quote_ident("a\"b"), "\"a\"\"b\"");
    }
}
