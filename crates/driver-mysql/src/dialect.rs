use driver_api::{Dialect, LexicalFeatures, TableRef};

#[derive(Debug, Default)]
pub struct MysqlDialect;

impl Dialect for MysqlDialect {
    fn name(&self) -> &'static str {
        "mysql"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn quote_literal(&self, value: &str) -> String {
        // NO_BACKSLASH_ESCAPES の有無に左右されないよう、バックスラッシュを含む値は 16 進で書く
        if value.contains('\\') {
            let hex: String = value.bytes().map(|b| format!("{b:02X}")).collect();
            return format!("CONVERT(0x{hex} USING utf8mb4)");
        }
        format!("'{}'", value.replace('\'', "''"))
    }

    fn lexical_features(&self) -> LexicalFeatures {
        LexicalFeatures {
            backtick_ident: true,
            hash_comment: true,
            backslash_escapes: true,
            ..LexicalFeatures::default()
        }
    }

    fn bind_text(&self, _n: usize, type_name: &str) -> String {
        // 位置指定の `?`(値は出てくる順に渡される)。整数や小数は、文字列のまま比較すると
        // 倍精度に丸められるため、型を明示して変換する
        let t = type_name.to_ascii_lowercase();
        let int = ["tinyint", "smallint", "mediumint", "int", "bigint"];
        if int.iter().any(|i| t.starts_with(i)) {
            let kind = if t.contains("unsigned") {
                "UNSIGNED"
            } else {
                "SIGNED"
            };
            return format!("CAST(? AS {kind})");
        }
        if t.starts_with("decimal") || t.starts_with("numeric") {
            let args = t
                .split_once('(')
                .and_then(|(_, rest)| rest.split_once(')'))
                .map_or("65,30", |(a, _)| a);
            return format!("CAST(? AS DECIMAL({args}))");
        }
        "?".into()
    }

    fn begin_sql(&self) -> &'static str {
        "START TRANSACTION"
    }

    fn null_safe_equals(&self, lhs: &str, rhs: &str) -> String {
        format!("{lhs} <=> {rhs}")
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
            "SET SESSION TRANSACTION {}",
            if read_only { "READ ONLY" } else { "READ WRITE" }
        ))
    }

    fn explain_sql(&self, sql: &str, analyze: bool) -> Option<String> {
        Some(if analyze {
            format!("EXPLAIN ANALYZE {sql}")
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
        let d = MysqlDialect;
        assert_eq!(d.quote_ident("a`b"), "`a``b`");
        assert_eq!(d.quote_literal("it's"), "'it''s'");
        assert_eq!(d.quote_literal("a\\b"), "CONVERT(0x615C62 USING utf8mb4)");
        assert_eq!(
            d.select_table_sql(&TableRef::new("shop", "my table"), Some(10)),
            "SELECT * FROM `shop`.`my table` LIMIT 10"
        );
        assert_eq!(d.null_safe_equals("`a`", "?"), "`a` <=> ?");
    }

    #[test]
    fn bound_values_are_converted_to_the_column_type() {
        let d = MysqlDialect;
        assert_eq!(d.bind_text(1, "bigint"), "CAST(? AS SIGNED)");
        assert_eq!(d.bind_text(1, "int unsigned"), "CAST(? AS UNSIGNED)");
        assert_eq!(d.bind_text(1, "decimal(20,6)"), "CAST(? AS DECIMAL(20,6))");
        assert_eq!(d.bind_text(1, "decimal"), "CAST(? AS DECIMAL(65,30))");
        assert_eq!(d.bind_text(1, "varchar(20)"), "?");
        assert_eq!(d.bind_text(1, "datetime(6)"), "?");
    }
}
