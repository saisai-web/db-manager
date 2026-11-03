/// 文字列やコメントの字句規則。DB の名前ではなく機能の有無で表し、
/// `core` が文分割(sqlscan)の規則へ変換する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LexicalFeatures {
    /// `$$ ... $$` と `$tag$ ... $tag$`
    pub dollar_quote: bool,
    /// `E'...'`
    pub e_strings: bool,
    /// 入れ子の `/* /* */ */`
    pub nested_block_comments: bool,
    /// `` `ident` ``
    pub backtick_ident: bool,
    /// `# コメント`
    pub hash_comment: bool,
    /// `[ident]`
    pub bracket_ident: bool,
    /// 通常の文字列内のバックスラッシュがエスケープ
    pub backslash_escapes: bool,
    /// `q'[...]'`(Oracle の引用符の指定つき文字列)
    pub q_quote: bool,
    /// PL/SQL のブロック(`BEGIN ... END;`、`CREATE PROCEDURE ... END;`)は `;` で分割せず、
    /// 単独行の `/` を文の終わりとする
    pub plsql: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

impl TableRef {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }
}

/// 構造化された実行計画の形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFormat {
    /// PostgreSQL の `EXPLAIN (FORMAT JSON)`
    PostgresJson,
    /// MySQL 8 の `EXPLAIN FORMAT=TREE` / `EXPLAIN ANALYZE`(インデントした木の文字列)
    MysqlTree,
    /// MariaDB の `EXPLAIN FORMAT=JSON` / `ANALYZE FORMAT=JSON`
    MariadbJson,
    /// SQL Server の `SET SHOWPLAN_XML` / `SET STATISTICS XML` が返す実行計画の XML
    SqlServerXml,
    /// Oracle の計画表(`PLAN_TABLE`、実測は `V$SQL_PLAN_STATISTICS_ALL`)の行を、ドライバが
    /// JSON の配列にしたもの(各要素は `id`、`parent_id`、`operation`、`options`、`object_name`、
    /// `cost`、`cardinality`、`access_predicates`、`filter_predicates`、実測なら
    /// `starts`、`output_rows`、`elapsed_us`)
    OracleJson,
}

/// DB ごとの SQL の書き方。
pub trait Dialect: Send + Sync {
    fn name(&self) -> &'static str;

    /// 識別子を引用符で囲む。内部の引用符はエスケープする。
    fn quote_ident(&self, ident: &str) -> String;

    /// 文字列リテラルにする。
    fn quote_literal(&self, value: &str) -> String;

    fn lexical_features(&self) -> LexicalFeatures;

    /// `schema.table` の形の、引用符つきの完全修飾名。
    fn qualified_name(&self, table: &TableRef) -> String {
        format!(
            "{}.{}",
            self.quote_ident(&table.schema),
            self.quote_ident(&table.name)
        )
    }

    /// `n` 番目(1 始まり)のバインド値を、型 `type_name` の値として使う式。
    /// 値は常に文字列で渡し、型への変換は DB に任せる(値の桁落ちや書式のずれを防ぐ)。
    fn bind_text(&self, n: usize, type_name: &str) -> String;

    /// トランザクションの開始・確定・取り消しの SQL。
    fn begin_sql(&self) -> &'static str {
        "BEGIN"
    }

    fn commit_sql(&self) -> &'static str {
        "COMMIT"
    }

    fn rollback_sql(&self) -> &'static str {
        "ROLLBACK"
    }

    /// NULL どうしも等しいとみなす比較(`a IS NOT DISTINCT FROM b` など)。
    /// 更新の WHERE に、変更した列の元の値を含めるときに使う(NULL でも一致させるため)。
    fn null_safe_equals(&self, lhs: &str, rhs: &str) -> String {
        format!("{lhs} IS NOT DISTINCT FROM {rhs}")
    }

    /// テーブルを閲覧する SELECT。`limit` が `None` なら全件。
    fn select_table_sql(&self, table: &TableRef, limit: Option<u64>) -> String;

    /// セッションの読み取り専用モードを切り替える SQL。切り替えられない DB は `None`。
    /// これは誤操作の防止であり、権限制御ではない。
    fn set_read_only_sql(&self, read_only: bool) -> Option<String>;

    /// 実行計画を、構造化された形(JSON)で取得する SQL。可視化に対応していない DB は `None`。
    fn explain_structured_sql(&self, _sql: &str, _analyze: bool) -> Option<(String, PlanFormat)> {
        None
    }

    /// 実行計画を取得する SQL。`analyze` が真なら実際に実行する。
    fn explain_sql(&self, sql: &str, analyze: bool) -> Option<String>;
}
