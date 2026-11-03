use driver_api::{Dialect, LexicalFeatures, TableRef};

#[derive(Debug, Default)]
pub struct OracleDialect;

impl Dialect for OracleDialect {
    fn name(&self) -> &'static str {
        "oracle"
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn quote_literal(&self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    fn lexical_features(&self) -> LexicalFeatures {
        LexicalFeatures {
            q_quote: true,
            plsql: true,
            ..LexicalFeatures::default()
        }
    }

    fn bind_text(&self, n: usize, type_name: &str) -> String {
        // 値は文字列で渡し、列の型に直す。接続のとき、書式を固定している(`session::SESSION_SETUP`)
        let t = type_name.to_ascii_uppercase();
        let head = t
            .split(|c: char| c == '(' || c.is_whitespace())
            .next()
            .unwrap_or("");
        match head {
            "NUMBER" | "FLOAT" | "INTEGER" | "INT" | "SMALLINT" | "DECIMAL" | "NUMERIC" => {
                format!("CAST(:{n} AS NUMBER)")
            }
            "BINARY_DOUBLE" => format!("CAST(:{n} AS BINARY_DOUBLE)"),
            "BINARY_FLOAT" => format!("CAST(:{n} AS BINARY_FLOAT)"),
            "DATE" => format!("TO_DATE(:{n}, 'YYYY-MM-DD HH24:MI:SS')"),
            "TIMESTAMP" if t.contains("TIME ZONE") && !t.contains("LOCAL") => {
                format!("TO_TIMESTAMP_TZ(:{n}, 'YYYY-MM-DD HH24:MI:SS.FF TZR')")
            }
            "TIMESTAMP" => format!("TO_TIMESTAMP(:{n}, 'YYYY-MM-DD HH24:MI:SS.FF')"),
            "RAW" => format!("HEXTORAW(:{n})"),
            _ => format!(":{n}"),
        }
    }

    /// Oracle には、トランザクションの開始の文がない(最初の文で暗黙に始まる)。いつでも実行でき、
    /// トランザクションを始める文として、セーブポイントを使う(`SET TRANSACTION` は、すでに
    /// トランザクションがあると失敗する)。
    fn begin_sql(&self) -> &'static str {
        "SAVEPOINT sqltool_begin"
    }

    /// `DECODE` は、NULL どうしを等しいとみなす。`rhs`(バインドの式)を 1 回しか書かないので、
    /// 位置指定のバインド(同じ位置でも、書いた数だけ値が要る)の数が、ずれない。
    fn null_safe_equals(&self, lhs: &str, rhs: &str) -> String {
        format!("DECODE({lhs}, {rhs}, 1, 0) = 1")
    }

    fn select_table_sql(&self, table: &TableRef, limit: Option<u64>) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name(table));
        if let Some(n) = limit {
            sql.push_str(&format!(" FETCH FIRST {n} ROWS ONLY"));
        }
        sql
    }

    /// Oracle には、セッション全体の読み取り専用がない(`SET TRANSACTION READ ONLY` は、
    /// トランザクション 1 回ぶんだけ)。
    fn set_read_only_sql(&self, _read_only: bool) -> Option<String> {
        None
    }

    fn explain_sql(&self, _sql: &str, _analyze: bool) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let d = OracleDialect;
        assert_eq!(d.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(d.quote_literal("it's"), "'it''s'");
        assert_eq!(d.quote_literal("a\\b"), "'a\\b'");
        assert_eq!(
            d.select_table_sql(&TableRef::new("SHOP", "my table"), Some(10)),
            "SELECT * FROM \"SHOP\".\"my table\" FETCH FIRST 10 ROWS ONLY"
        );
    }

    #[test]
    fn bound_values_are_converted_to_the_column_type() {
        let d = OracleDialect;
        assert_eq!(d.bind_text(1, "NUMBER(10,2)"), "CAST(:1 AS NUMBER)");
        assert_eq!(d.bind_text(2, "NUMBER"), "CAST(:2 AS NUMBER)");
        assert_eq!(d.bind_text(1, "VARCHAR2(20)"), ":1");
        assert_eq!(
            d.bind_text(1, "DATE"),
            "TO_DATE(:1, 'YYYY-MM-DD HH24:MI:SS')"
        );
        assert_eq!(
            d.bind_text(3, "TIMESTAMP(6)"),
            "TO_TIMESTAMP(:3, 'YYYY-MM-DD HH24:MI:SS.FF')"
        );
        assert_eq!(
            d.bind_text(3, "TIMESTAMP(6) WITH TIME ZONE"),
            "TO_TIMESTAMP_TZ(:3, 'YYYY-MM-DD HH24:MI:SS.FF TZR')"
        );
        assert_eq!(d.bind_text(1, "RAW(16)"), "HEXTORAW(:1)");
    }

    #[test]
    fn null_safe_comparison_and_transactions() {
        let d = OracleDialect;
        assert_eq!(
            d.null_safe_equals("\"A\"", ":1"),
            "DECODE(\"A\", :1, 1, 0) = 1"
        );
        assert_eq!(d.set_read_only_sql(true), None);
        assert_eq!(d.begin_sql(), "SAVEPOINT sqltool_begin");
    }
}
