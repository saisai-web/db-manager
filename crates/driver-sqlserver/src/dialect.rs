use driver_api::{Dialect, LexicalFeatures, TableRef};

#[derive(Debug, Default)]
pub struct SqlServerDialect;

impl Dialect for SqlServerDialect {
    fn name(&self) -> &'static str {
        "sqlserver"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("[{}]", ident.replace(']', "]]"))
    }

    fn quote_literal(&self, value: &str) -> String {
        // 日本語などを失わないよう、Unicode リテラルにする。バックスラッシュは通常の文字
        format!("N'{}'", value.replace('\'', "''"))
    }

    fn lexical_features(&self) -> LexicalFeatures {
        LexicalFeatures {
            bracket_ident: true,
            nested_block_comments: true,
            ..LexicalFeatures::default()
        }
    }

    fn bind_text(&self, n: usize, type_name: &str) -> String {
        // パラメータは文字列(nvarchar)で送り、列の型へ明示的に変換する。型ごとの表現を作らずに済み、
        // bigint や decimal も桁落ちしない
        let t = type_name.to_ascii_lowercase();
        let head = t.split(['(', ' ']).next().unwrap_or("");
        if matches!(head, "binary" | "varbinary" | "image") {
            // `0x...` の 16 進文字列から、バイト列へ
            return format!("CONVERT({}, @P{n}, 1)", cast_type(&t));
        }
        format!("CAST(@P{n} AS {})", cast_type(&t))
    }

    fn begin_sql(&self) -> &'static str {
        "BEGIN TRANSACTION"
    }

    fn null_safe_equals(&self, lhs: &str, rhs: &str) -> String {
        format!("({lhs} = {rhs} OR ({lhs} IS NULL AND {rhs} IS NULL))")
    }

    fn select_table_sql(&self, table: &TableRef, limit: Option<u64>) -> String {
        let top = limit.map_or(String::new(), |n| format!("TOP ({n}) "));
        format!("SELECT {top}* FROM {}", self.qualified_name(table))
    }

    /// SQL Server には、セッションを読み取り専用にする SQL が無い。セッション自身が、文ごとの
    /// トランザクションで実現する(`Session::set_read_only`、`readonly` モジュール)。
    fn set_read_only_sql(&self, _read_only: bool) -> Option<String> {
        None
    }

    fn explain_sql(&self, _sql: &str, _analyze: bool) -> Option<String> {
        None
    }
}

/// `CAST` に使える型名にする(メタデータの `nvarchar(-1)` などは `max`)。
fn cast_type(t: &str) -> String {
    let t = t.replace("(-1)", "(max)");
    match t.as_str() {
        // 長さの無い文字型は 1 文字になってしまう。長さを付ける
        "varchar" | "nvarchar" | "varbinary" => format!("{t}(max)"),
        "binary" | "char" | "nchar" => format!("{t}(8000)"),
        _ => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let d = SqlServerDialect;
        assert_eq!(d.quote_ident("a]b"), "[a]]b]");
        assert_eq!(d.quote_literal("it's"), "N'it''s'");
        assert_eq!(d.quote_literal("a\\b"), "N'a\\b'");
        assert_eq!(
            d.select_table_sql(&TableRef::new("dbo", "my table"), Some(10)),
            "SELECT TOP (10) * FROM [dbo].[my table]"
        );
        assert_eq!(
            d.select_table_sql(&TableRef::new("dbo", "t"), None),
            "SELECT * FROM [dbo].[t]"
        );
        assert_eq!(
            d.null_safe_equals("[a]", "@P1"),
            "([a] = @P1 OR ([a] IS NULL AND @P1 IS NULL))"
        );
        assert!(d.set_read_only_sql(true).is_none());
    }

    #[test]
    fn bound_values_are_converted_to_the_column_type() {
        let d = SqlServerDialect;
        assert_eq!(d.bind_text(1, "bigint"), "CAST(@P1 AS bigint)");
        assert_eq!(
            d.bind_text(2, "decimal(20,6)"),
            "CAST(@P2 AS decimal(20,6))"
        );
        assert_eq!(d.bind_text(3, "nvarchar(-1)"), "CAST(@P3 AS nvarchar(max))");
        assert_eq!(d.bind_text(4, "varchar"), "CAST(@P4 AS varchar(max))");
        assert_eq!(
            d.bind_text(5, "varbinary(16)"),
            "CONVERT(varbinary(16), @P5, 1)"
        );
    }
}
