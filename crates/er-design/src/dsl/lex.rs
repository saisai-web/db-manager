//! 1 行を語に分ける。

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// 空白で区切られた語。`varchar(100)` や `now()` のように、直後の括弧は語に含む
    Word(String),
    /// `"..."`(論理名・備考)。エスケープは解いてある
    Quoted(String),
    /// `'...'`(SQL の文字列。引用符つきのまま)
    Lit(String),
    /// `` `...` ``(空白を含む SQL の式。引用符は外してある)
    Raw(String),
    /// 語と離れた括弧 `(a, b)`
    Paren(Vec<String>),
}

pub fn split_list(inner: &str) -> Vec<String> {
    inner
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn tokenize(line: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '"' => {
                let (s, next) = quoted(&chars, i)?;
                toks.push(Tok::Quoted(s));
                i = next;
            }
            '\'' => {
                let (s, next) = single_quoted(&chars, i)?;
                toks.push(Tok::Lit(s));
                i = next;
            }
            '`' => {
                let end = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '`')
                    .ok_or("` が閉じられていません")?;
                toks.push(Tok::Raw(chars[i + 1..i + 1 + end].iter().collect()));
                i += end + 2;
            }
            '(' => {
                let end = matching_paren(&chars, i)?;
                let inner: String = chars[i + 1..end].iter().collect();
                toks.push(Tok::Paren(split_list(&inner)));
                i = end + 1;
            }
            _ => {
                let start = i;
                while i < chars.len() && !chars[i].is_whitespace() && !"\"'`".contains(chars[i]) {
                    if chars[i] == '(' {
                        i = matching_paren(&chars, i)? + 1;
                    } else {
                        i += 1;
                    }
                }
                toks.push(Tok::Word(chars[start..i].iter().collect()));
            }
        }
    }
    Ok(toks)
}

fn matching_paren(chars: &[char], open: usize) -> Result<usize, String> {
    let mut depth = 0;
    for (i, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err("( が閉じられていません".into())
}

/// `"..."` を読む。`\"`、`\\`、`\n` を解く。
fn quoted(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '"' => return Ok((out, i + 1)),
            '\\' if i + 1 < chars.len() => {
                out.push(match chars[i + 1] {
                    'n' => '\n',
                    c => c,
                });
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Err("\" が閉じられていません".into())
}

/// `'...'` を読む(`''` は引用符 1 文字)。引用符つきのまま返す。
fn single_quoted(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                i += 2;
                continue;
            }
            return Ok((chars[start..=i].iter().collect(), i + 1));
        }
        i += 1;
    }
    Err("' が閉じられていません".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_strings_and_parens() {
        assert_eq!(
            tokenize("name varchar(100) not null \"氏 名\" 'a b' (x, y)").unwrap(),
            [
                Tok::Word("name".into()),
                Tok::Word("varchar(100)".into()),
                Tok::Word("not".into()),
                Tok::Word("null".into()),
                Tok::Quoted("氏 名".into()),
                Tok::Lit("'a b'".into()),
                Tok::Paren(vec!["x".into(), "y".into()]),
            ]
        );
    }

    #[test]
    fn keeps_spaces_inside_attached_parens_and_unescapes_strings() {
        assert_eq!(
            tokenize("decimal(10, 2) \"a\\\"b\\n\" 'it''s' `now() + 1`").unwrap(),
            [
                Tok::Word("decimal(10, 2)".into()),
                Tok::Quoted("a\"b\n".into()),
                Tok::Lit("'it''s'".into()),
                Tok::Raw("now() + 1".into()),
            ]
        );
    }

    #[test]
    fn unclosed_quotes_and_parens_are_errors() {
        for bad in ["a \"b", "a 'b", "a `b", "a (b", "a b(c"] {
            assert!(tokenize(bad).is_err(), "{bad}");
        }
    }
}
