use driver_api::{Dialect, LexicalFeatures, PlanFormat, TableRef};

#[derive(Debug, Default)]
pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "postgresql"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn quote_literal(&self, value: &str) -> String {
        // standard_conforming_strings = on(既定)ではバックスラッシュは通常の文字。
        // 設定に左右されないよう、バックスラッシュを含む場合は E'' で書く。
        let escaped = value.replace('\'', "''");
        if value.contains('\\') {
            format!("E'{}'", escaped.replace('\\', "\\\\"))
        } else {
            format!("'{escaped}'")
        }
    }

    fn lexical_features(&self) -> LexicalFeatures {
        LexicalFeatures {
            dollar_quote: true,
            e_strings: true,
            nested_block_comments: true,
            ..LexicalFeatures::default()
        }
    }

    fn bind_text(&self, n: usize, type_name: &str) -> String {
        // パラメータは text として送り、列の型へ変換する。型ごとのバイナリ表現を作らずに済み、
        // numeric や timestamptz も文字列のまま正確に渡せる
        format!("(${n}::text)::{type_name}")
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
            "SET SESSION CHARACTERISTICS AS TRANSACTION {}",
            if read_only { "READ ONLY" } else { "READ WRITE" }
        ))
    }

    fn explain_structured_sql(&self, sql: &str, analyze: bool) -> Option<(String, PlanFormat)> {
        let options = if analyze {
            "FORMAT JSON, ANALYZE, BUFFERS"
        } else {
            "FORMAT JSON"
        };
        Some((
            format!("EXPLAIN ({options}) {sql}"),
            PlanFormat::PostgresJson,
        ))
    }

    fn explain_sql(&self, sql: &str, analyze: bool) -> Option<String> {
        Some(if analyze {
            format!("EXPLAIN (ANALYZE, BUFFERS) {sql}")
        } else {
            format!("EXPLAIN {sql}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let d = PostgresDialect;
        assert_eq!(d.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(d.quote_literal("it's"), "'it''s'");
        assert_eq!(d.quote_literal("a\\b"), "E'a\\\\b'");
        assert_eq!(d.quote_literal("a\\'b"), "E'a\\\\''b'");
        assert_eq!(d.bind_text(2, "numeric(12,2)"), "($2::text)::numeric(12,2)");
        assert_eq!(
            d.select_table_sql(&TableRef::new("public", "my table"), Some(10)),
            "SELECT * FROM \"public\".\"my table\" LIMIT 10"
        );
    }
}
