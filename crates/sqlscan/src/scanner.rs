/// DB ごとの字句ルール。共通部分(文字列、引用符付き識別子、コメント)は常に有効。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// `$$ ... $$` と `$tag$ ... $tag$`
    pub dollar_quote: bool,
    /// `E'...'`(バックスラッシュでエスケープできる文字列)
    pub e_strings: bool,
    /// `/* /* */ */` を入れ子として扱う
    pub nested_block_comments: bool,
    /// `` `ident` ``
    pub backtick_ident: bool,
    /// `# コメント`
    pub hash_comment: bool,
    /// `[ident]`
    pub bracket_ident: bool,
    /// 通常の文字列内でもバックスラッシュがエスケープになる
    pub backslash_escapes: bool,
    /// `q'[...]'`(Oracle の引用符の指定つき文字列)
    pub q_quote: bool,
    /// PL/SQL のブロック(`BEGIN ... END;`)は `;` で分割せず、単独行の `/` も文の終わりとする
    pub plsql: bool,
}

impl Rules {
    /// 標準 SQL の共通ルールだけを持つ。
    pub const fn generic() -> Self {
        Self {
            dollar_quote: false,
            e_strings: false,
            nested_block_comments: false,
            backtick_ident: false,
            hash_comment: false,
            bracket_ident: false,
            backslash_escapes: false,
            q_quote: false,
            plsql: false,
        }
    }

    pub const fn postgres() -> Self {
        Self {
            dollar_quote: true,
            e_strings: true,
            nested_block_comments: true,
            ..Self::generic()
        }
    }
}

impl Default for Rules {
    fn default() -> Self {
        Self::postgres()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Whitespace,
    LineComment,
    BlockComment,
    /// 識別子またはキーワード
    Word,
    /// `"x"`、`` `x` ``、`[x]`
    QuotedIdent,
    /// `'x'`、`E'x'`
    String,
    /// `$$x$$`、`$tag$x$tag$`
    DollarString,
    Number,
    /// 1 文字の記号。`;` `(` `)` を含む
    Punct,
}

impl TokenKind {
    /// 空白とコメント。文の内容には含まれない。
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace | Self::LineComment | Self::BlockComment
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// 入力文字列内のバイト位置(常に文字境界)
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.start..self.end]
    }
}

fn is_word_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_word_continue(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

/// 閉じていない文字列やコメントは入力の終端までを 1 つのトークンとする。
/// どんな入力でも panic せず、トークンは入力を隙間なく覆う。
pub fn tokenize(sql: &str, rules: &Rules) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < sql.len() {
        let (kind, end) = next_token(sql, pos, rules);
        debug_assert!(end > pos && sql.is_char_boundary(end));
        tokens.push(Token {
            kind,
            start: pos,
            end,
        });
        pos = end;
    }
    tokens
}

fn next_token(sql: &str, pos: usize, rules: &Rules) -> (TokenKind, usize) {
    let rest = &sql[pos..];
    let mut chars = rest.chars();
    let c = chars.next().expect("pos < len");
    let after_c = pos + c.len_utf8();
    let next = chars.next();

    if c.is_whitespace() {
        let len = rest
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(rest.len());
        return (TokenKind::Whitespace, pos + len);
    }

    if c == '-' && next == Some('-') {
        return (TokenKind::LineComment, line_end(sql, pos));
    }
    if c == '#' && rules.hash_comment {
        return (TokenKind::LineComment, line_end(sql, pos));
    }
    if c == '/' && next == Some('*') {
        return (
            TokenKind::BlockComment,
            block_comment_end(sql, pos, rules.nested_block_comments),
        );
    }

    match c {
        '\'' => {
            return (
                TokenKind::String,
                quoted_end(sql, after_c, '\'', rules.backslash_escapes),
            );
        }
        '"' => return (TokenKind::QuotedIdent, quoted_end(sql, after_c, '"', false)),
        '`' if rules.backtick_ident => {
            return (TokenKind::QuotedIdent, quoted_end(sql, after_c, '`', false));
        }
        '[' if rules.bracket_ident => {
            let end = sql[after_c..]
                .find(']')
                .map_or(sql.len(), |i| after_c + i + 1);
            return (TokenKind::QuotedIdent, end);
        }
        '$' if rules.dollar_quote => {
            if let Some(end) = dollar_quote_end(sql, pos) {
                return (TokenKind::DollarString, end);
            }
        }
        _ => {}
    }

    if c.is_ascii_digit() {
        let len = rest
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '.' || ch == '_'))
            .unwrap_or(rest.len());
        return (TokenKind::Number, pos + len);
    }

    if is_word_start(c) {
        let len = rest
            .find(|ch: char| !is_word_continue(ch))
            .unwrap_or(rest.len());
        let end = pos + len;
        // E'...' は単独の E の直後に引用符が続く場合だけ
        if rules.q_quote
            && matches!(&sql[pos..end], "q" | "Q" | "nq" | "NQ" | "nQ" | "Nq")
            && let Some(string_end) = q_quote_end(sql, end)
        {
            return (TokenKind::String, string_end);
        }
        if rules.e_strings && len == 1 && (c == 'E' || c == 'e') && sql[end..].starts_with('\'') {
            return (TokenKind::String, quoted_end(sql, end + 1, '\'', true));
        }
        return (TokenKind::Word, end);
    }

    (TokenKind::Punct, after_c)
}

/// `from` は `q` の直後。`'` に区切り文字が続く形なら、閉じ(`区切り'`)の直後の位置を返す。
/// 区切りが `[` `{` `(` `<` のときは、対応する閉じ括弧。閉じていなければ入力の終端まで。
fn q_quote_end(sql: &str, from: usize) -> Option<usize> {
    let rest = sql[from..].strip_prefix('\'')?;
    let open = rest.chars().next().filter(|c| !c.is_whitespace())?;
    let close = match open {
        '[' => ']',
        '{' => '}',
        '(' => ')',
        '<' => '>',
        c => c,
    };
    let body = from + 1 + open.len_utf8();
    let pattern = format!("{close}'");
    Some(
        sql[body..]
            .find(&pattern)
            .map_or(sql.len(), |i| body + i + pattern.len()),
    )
}

fn line_end(sql: &str, pos: usize) -> usize {
    sql[pos..].find('\n').map_or(sql.len(), |i| pos + i)
}

fn block_comment_end(sql: &str, pos: usize, nested: bool) -> usize {
    let bytes = sql.as_bytes();
    let mut i = pos + 2;
    let mut depth = 1usize;
    while i < bytes.len() {
        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else if nested && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    sql.len()
}

/// `from` は開き引用符の直後。閉じ引用符の直後の位置を返す。引用符の二重化は
/// エスケープ、`backslash` が真ならバックスラッシュも次の 1 文字をエスケープする。
fn quoted_end(sql: &str, from: usize, quote: char, backslash: bool) -> usize {
    let mut iter = sql[from..].char_indices();
    while let Some((i, ch)) = iter.next() {
        if backslash && ch == '\\' {
            iter.next();
        } else if ch == quote {
            let after = from + i + ch.len_utf8();
            if sql[after..].starts_with(quote) {
                iter.next();
            } else {
                return after;
            }
        }
    }
    sql.len()
}

/// `pos` が `$` のとき、`$tag$ ... $tag$` の終端を返す。開きタグが成立しなければ None。
/// 閉じていなければ入力の終端まで。
fn dollar_quote_end(sql: &str, pos: usize) -> Option<usize> {
    let after_dollar = pos + 1;
    let tag_len = sql[after_dollar..]
        .find(|ch: char| !is_word_continue(ch) || ch == '$')
        .unwrap_or(sql.len() - after_dollar);
    let tag = &sql[after_dollar..after_dollar + tag_len];
    if tag.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None; // $1 はパラメータ
    }
    if !sql[after_dollar + tag_len..].starts_with('$') {
        return None;
    }
    let open_end = after_dollar + tag_len + 1;
    let delim = &sql[pos..open_end];
    Some(
        sql[open_end..]
            .find(delim)
            .map_or(sql.len(), |i| open_end + i + delim.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str, rules: &Rules) -> Vec<(TokenKind, String)> {
        tokenize(sql, rules)
            .into_iter()
            .filter(|t| !t.kind.is_trivia())
            .map(|t| (t.kind, t.text(sql).to_string()))
            .collect()
    }

    #[test]
    fn strings_double_the_quote() {
        let k = kinds("select 'it''s'", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::String, "'it''s'".into()));
    }

    #[test]
    fn standard_string_ignores_backslash_in_postgres() {
        let k = kinds(r"select 'a\' , 1", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::String, r"'a\'".into()));
    }

    #[test]
    fn e_string_honors_backslash() {
        let k = kinds(r"select E'a\'b', 1", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::String, r"E'a\'b'".into()));
        assert_eq!(k[2], (TokenKind::Punct, ",".into()));
    }

    #[test]
    fn word_ending_in_e_is_not_an_e_string() {
        let k = kinds("select name'x'", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::Word, "name".into()));
        assert_eq!(k[2].0, TokenKind::String);
    }

    #[test]
    fn dollar_quotes_with_and_without_tag() {
        let k = kinds("select $$a;b$$, $f$ $$ ; $f$", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::DollarString, "$$a;b$$".into()));
        assert_eq!(k[3], (TokenKind::DollarString, "$f$ $$ ; $f$".into()));
    }

    #[test]
    fn dollar_param_is_not_a_quote() {
        let k = kinds("select $1, $2", &Rules::postgres());
        assert_eq!(k[1].0, TokenKind::Punct);
        assert_eq!(k[2].0, TokenKind::Number);
    }

    #[test]
    fn dollar_inside_identifier_stays_in_identifier() {
        let k = kinds("select a$b$ from t", &Rules::postgres());
        assert_eq!(k[1], (TokenKind::Word, "a$b$".into()));
    }

    #[test]
    fn oracle_q_quotes() {
        let rules = Rules {
            q_quote: true,
            ..Rules::generic()
        };
        let k = kinds("select q'[it's; ok]', Q'!a!!b!', nq'(x)' , 1", &rules);
        assert_eq!(k[1], (TokenKind::String, "q'[it's; ok]'".into()));
        assert_eq!(k[3], (TokenKind::String, "Q'!a!!b!'".into()));
        assert_eq!(k[5], (TokenKind::String, "nq'(x)'".into()));
        // q の後が空白の `q' '` は、ふつうの文字列(q は別の語)
        let k = kinds("select q' ', 1", &rules);
        assert_eq!(k[1], (TokenKind::Word, "q".into()));
        // 規則が無効なら、ふつうの文字列
        let k = kinds("select q'[a]'", &Rules::generic());
        assert_eq!(k[1], (TokenKind::Word, "q".into()));
    }

    #[test]
    fn nested_block_comments_only_when_enabled() {
        let pg = kinds("/* a /* b */ c */ x", &Rules::postgres());
        assert_eq!(pg, [(TokenKind::Word, "x".to_string())]);
        // 入れ子を認めないと最初の */ で閉じ、残りが通常のトークンになる
        let generic = kinds("/* a /* b */ c */ x", &Rules::generic());
        assert_eq!(generic[0], (TokenKind::Word, "c".to_string()));
    }

    #[test]
    fn quoted_identifier_with_semicolon() {
        let k = kinds(r#"select "a;""b" from t"#, &Rules::postgres());
        assert_eq!(k[1], (TokenKind::QuotedIdent, r#""a;""b""#.into()));
    }

    #[test]
    fn mysql_style_rules() {
        let rules = Rules {
            backtick_ident: true,
            hash_comment: true,
            backslash_escapes: true,
            ..Rules::generic()
        };
        let k = kinds("select `a;b`, 'x\\'y' # c;\n, 1", &rules);
        assert_eq!(k[1], (TokenKind::QuotedIdent, "`a;b`".into()));
        assert_eq!(k[3], (TokenKind::String, "'x\\'y'".into()));
        assert_eq!(k[4], (TokenKind::Punct, ",".into()));
    }

    #[test]
    fn sqlserver_bracket_identifier() {
        let rules = Rules {
            bracket_ident: true,
            ..Rules::generic()
        };
        let k = kinds("select [a;b] from t", &rules);
        assert_eq!(k[1], (TokenKind::QuotedIdent, "[a;b]".into()));
    }

    #[test]
    fn unterminated_constructs_run_to_end() {
        for sql in ["'abc", "\"abc", "/* abc", "$$abc", "E'abc\\"] {
            let t = tokenize(sql, &Rules::postgres());
            assert_eq!(t.last().unwrap().end, sql.len(), "{sql}");
        }
    }

    #[test]
    fn multibyte_text_is_handled_on_char_boundaries() {
        let sql = "select '日本語', 列 from 表 -- コメント";
        let t = tokenize(sql, &Rules::postgres());
        let rebuilt: String = t.iter().map(|t| t.text(sql)).collect();
        assert_eq!(rebuilt, sql);
        assert!(
            t.iter()
                .any(|t| t.kind == TokenKind::Word && t.text(sql) == "列")
        );
    }
}
