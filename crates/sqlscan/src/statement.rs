use std::ops::Range;

use crate::scanner::{Rules, Token, TokenKind, tokenize};

/// 安全確認とモード判定に使う、文の大まかな種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// SELECT / VALUES / TABLE
    Select,
    Insert,
    Update,
    Delete,
    Merge,
    Create,
    Alter,
    Drop,
    Truncate,
    /// BEGIN / COMMIT / ROLLBACK / SAVEPOINT など
    Transaction,
    /// SHOW / EXPLAIN
    Inspect,
    /// SET / RESET。セッションの設定を変える
    Session,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 先頭から末尾までの有効なトークンの範囲。前後の空白、コメント、終端の `;` は含まない。
    pub range: Range<usize>,
    /// 終端の `;` を含めた末尾。`;` がなければ `range.end` と同じ。
    pub end_with_terminator: usize,
    /// 先頭キーワード(大文字)。`WITH` で始まる文では、CTE の後ろにある本体の
    /// キーワード(`WITH x AS (...) DELETE ...` なら `DELETE`)。
    pub keyword: Option<String>,
    /// 先頭キーワードの次の語(大文字)。`DROP TABLE` の `TABLE` など。
    pub second_keyword: Option<String>,
    pub kind: StatementKind,
    /// 括弧の外に `WHERE` がある。UPDATE / DELETE の確認判定に使う。
    pub has_where: bool,
    /// `WITH` の括弧内に INSERT / UPDATE / DELETE / MERGE がある(データ変更 CTE)。
    pub nested_write: bool,
    /// パスワードを含みうる。履歴に保存してはならない。
    pub may_contain_password: bool,
    /// `EXPLAIN ANALYZE` のように、与えた文を実際に実行する。
    pub analyze: bool,
    /// `BEGIN READ WRITE` のように、トランザクションを書き込み可能にする。
    writes_transaction: bool,
    /// `set_config(...)` を呼ぶ。SELECT でもセッションの設定(読み取り専用の解除を含む)を変えられる。
    changes_session: bool,
}

impl Statement {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.range.clone()]
    }

    /// データやスキーマ、セッション設定を変更しないと字句の範囲で判断できる文。
    /// 権限制御ではなく誤操作の防止のための判定で、DB 側の読み取り専用設定と併用する。
    pub fn is_read_only(&self) -> bool {
        if self.nested_write || self.changes_session {
            return false;
        }
        match self.kind {
            StatementKind::Select => true,
            StatementKind::Inspect => !self.analyze,
            StatementKind::Transaction => !self.writes_transaction,
            _ => false,
        }
    }

    /// WHERE を持たない UPDATE / DELETE。
    pub fn is_unfiltered_dml(&self) -> bool {
        matches!(self.kind, StatementKind::Update | StatementKind::Delete) && !self.has_where
    }

    /// DROP / TRUNCATE
    pub fn is_destructive_ddl(&self) -> bool {
        matches!(self.kind, StatementKind::Drop | StatementKind::Truncate)
    }
}

fn classify(keyword: &str) -> StatementKind {
    match keyword {
        "SELECT" | "VALUES" | "TABLE" => StatementKind::Select,
        "INSERT" => StatementKind::Insert,
        "UPDATE" => StatementKind::Update,
        "DELETE" => StatementKind::Delete,
        "MERGE" => StatementKind::Merge,
        "CREATE" => StatementKind::Create,
        "ALTER" => StatementKind::Alter,
        "DROP" => StatementKind::Drop,
        "TRUNCATE" => StatementKind::Truncate,
        "BEGIN" | "START" | "COMMIT" | "END" | "ROLLBACK" | "ABORT" | "SAVEPOINT" | "RELEASE" => {
            StatementKind::Transaction
        }
        "SHOW" | "EXPLAIN" => StatementKind::Inspect,
        "SET" | "RESET" => StatementKind::Session,
        _ => StatementKind::Other,
    }
}

/// SQL を文に分割する。空の文(`;` だけ、コメントだけ)は返さない。
///
/// 文の区切りは括弧の外の `;` に限る。`CREATE RULE ... DO ALSO (a; b)` や
/// `BEGIN ATOMIC a; b; END`(PostgreSQL 14 以降の SQL 関数本体)の途中では分割しない。
pub fn split(sql: &str, rules: &Rules) -> Vec<Statement> {
    let tokens = tokenize(sql, rules);
    let mut out = Vec::new();
    let mut acc = Accumulator::default();

    for tok in &tokens {
        if tok.kind.is_trivia() {
            continue;
        }
        if tok.kind == TokenKind::Punct && tok.text(sql) == ";" && acc.at_boundary() {
            if let Some(st) = acc.finish(tok.end) {
                out.push(st);
            }
            acc = Accumulator::default();
            continue;
        }
        acc.push(sql, tok);
    }
    if let Some(st) = acc.finish(0) {
        out.push(st);
    }
    out
}

/// カーソル位置(バイト)に対応する文。
///
/// 文の先頭から終端の `;` まで(両端を含む)にカーソルがあればその文。文と文の間に
/// あれば直前の文、先頭の文より前なら最初の文を返す。
pub fn statement_at(statements: &[Statement], offset: usize) -> Option<&Statement> {
    let mut previous = None;
    for st in statements {
        if offset < st.range.start {
            return previous.or(Some(st));
        }
        if offset <= st.end_with_terminator {
            return Some(st);
        }
        previous = Some(st);
    }
    previous
}

#[derive(Default)]
struct Accumulator {
    start: Option<usize>,
    end: usize,
    keyword: Option<String>,
    second_keyword: Option<String>,
    /// 文が `WITH` で始まる
    is_with: bool,
    /// `WITH` の本体のキーワードをまだ見ていない
    awaiting_body: bool,
    depth: usize,
    has_where: bool,
    nested_write: bool,
    may_contain_password: bool,
    analyze: bool,
    writes_transaction: bool,
    changes_session: bool,
    prev_word: Option<String>,
    in_atomic: bool,
    case_depth: usize,
    /// 先頭から数えた語の数(キーワード判定用。括弧の外のみ)
    top_words: usize,
}

impl Accumulator {
    fn at_boundary(&self) -> bool {
        self.depth == 0 && !self.in_atomic
    }

    fn push(&mut self, sql: &str, tok: &Token) {
        if self.start.is_none() {
            self.start = Some(tok.start);
        }
        self.end = tok.end;

        match tok.kind {
            TokenKind::Punct => match tok.text(sql) {
                "(" => self.depth += 1,
                ")" => self.depth = self.depth.saturating_sub(1),
                _ => {}
            },
            TokenKind::String | TokenKind::DollarString => {
                if text_mentions_password(tok.text(sql)) {
                    self.may_contain_password = true;
                }
            }
            TokenKind::Word => self.push_word(&tok.text(sql).to_ascii_uppercase()),
            _ => {}
        }
        if tok.kind != TokenKind::Word {
            self.prev_word = None;
        }
    }

    fn push_word(&mut self, word: &str) {
        if matches!(word, "PASSWORD" | "ENCRYPTED" | "IDENTIFIED") {
            self.may_contain_password = true;
        }
        if matches!(word, "ANALYZE" | "ANALYSE") {
            self.analyze = true;
        }
        if word == "WRITE" {
            self.writes_transaction = true;
        }
        if word == "SET_CONFIG" {
            self.changes_session = true;
        }

        // BEGIN ATOMIC ... END
        if self.in_atomic {
            match word {
                "CASE" => self.case_depth += 1,
                "END" if self.case_depth > 0 => self.case_depth -= 1,
                "END" => self.in_atomic = false,
                _ => {}
            }
        } else if word == "ATOMIC" && self.prev_word.as_deref() == Some("BEGIN") {
            self.in_atomic = true;
            self.case_depth = 0;
        }

        if self.depth == 0 {
            self.top_words += 1;
            match self.top_words {
                1 => {
                    self.is_with = word == "WITH";
                    self.awaiting_body = self.is_with;
                    self.keyword = Some(word.to_string());
                }
                2 => self.second_keyword = Some(word.to_string()),
                _ => {}
            }
            if self.awaiting_body
                && matches!(
                    word,
                    "SELECT" | "VALUES" | "TABLE" | "INSERT" | "UPDATE" | "DELETE" | "MERGE"
                )
            {
                self.awaiting_body = false;
                self.keyword = Some(word.to_string());
            }
            if word == "WHERE" {
                self.has_where = true;
            }
        } else if self.is_with && matches!(word, "INSERT" | "UPDATE" | "DELETE" | "MERGE") {
            self.nested_write = true;
        }
        self.prev_word = Some(word.to_string());
    }

    fn finish(self, terminator_end: usize) -> Option<Statement> {
        let start = self.start?;
        let keyword = self.keyword;
        let kind = keyword.as_deref().map_or(StatementKind::Other, classify);
        let may_contain_password = self.may_contain_password
            || (matches!(kind, StatementKind::Create | StatementKind::Alter)
                && matches!(
                    self.second_keyword.as_deref(),
                    Some("USER" | "ROLE" | "GROUP" | "SERVER" | "SUBSCRIPTION")
                ));
        Some(Statement {
            range: start..self.end,
            end_with_terminator: terminator_end.max(self.end),
            keyword,
            second_keyword: self.second_keyword,
            kind,
            has_where: self.has_where,
            nested_write: self.nested_write,
            may_contain_password,
            analyze: self.analyze,
            writes_transaction: self.writes_transaction,
            changes_session: self.changes_session,
        })
    }
}

fn text_mentions_password(literal: &str) -> bool {
    literal.to_ascii_lowercase().contains("password")
}
