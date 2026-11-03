//! SQL の補完候補。カーソル位置の文脈(テーブル名の位置か、`別名.` の後か、式の中か)で絞り込む。
//! スキーマ情報(カタログ)は、呼び出し側がキャッシュから渡す。入力中に DB へは問い合わせない。
//! DB に依存しない純粋なロジック。

use std::ops::Range;

use sqlscan::{Rules, TokenKind, split, tokenize};

mod context;
mod keywords;
#[cfg(test)]
mod tests;

use context::{Context, context, tables_in_statement};
use keywords::{KEYWORDS, STATEMENT_STARTERS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Keyword,
    Schema,
    Table,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub kind: CandidateKind,
    /// 補足(列の型、表のスキーマなど)
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// 候補で置き換える、入力中の語(バイト位置)。カーソルの位置で終わる
    pub replace: Range<usize>,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub tables: Vec<CatalogTable>,
}

#[derive(Debug, Clone)]
pub struct CatalogTable {
    pub schema: String,
    pub name: String,
    /// (列名, 型)。まだ読み込んでいなければ空
    pub columns: Vec<(String, String)>,
}

const MAX_CANDIDATES: usize = 50;

/// 補完の対象になる語の字句。
fn is_word_text(s: &str) -> bool {
    s.chars()
        .all(|c| c == '_' || c.is_alphanumeric() || c == '$')
}

fn matches_prefix(label: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(1);
    }
    let (l, p) = (label.to_lowercase(), prefix.to_lowercase());
    if l.starts_with(&p) {
        Some(0)
    } else if l.contains(&p) {
        Some(2)
    } else {
        None
    }
}

/// 打っている語の大文字小文字に合わせて、キーワードを書く(小文字で打っていれば小文字)。
fn keyword_case(kw: &str, prefix: &str) -> String {
    if !prefix.is_empty() && prefix.chars().all(|c| !c.is_uppercase()) {
        kw.to_lowercase()
    } else {
        kw.to_string()
    }
}

pub fn suggest(sql: &str, offset: usize, rules: &Rules, catalog: &Catalog) -> Suggestion {
    let offset = offset.min(sql.len());
    let statements = split(sql, rules);
    let (base, text) = match sqlscan::statement_at(&statements, offset) {
        // 終端の `;` がない文は、末尾に空白があっても、まだ入力の途中
        Some(st) if st.end_with_terminator == st.range.end => (
            st.range.start,
            &sql[st.range.start..offset.max(st.range.end)],
        ),
        Some(st) if offset <= st.end_with_terminator => (st.range.start, &sql[st.range.clone()]),
        // `;` の後は、新しい文の入力
        Some(st) => (st.end_with_terminator, &sql[st.end_with_terminator..]),
        None => (0, sql),
    };
    let rel = offset.saturating_sub(base).min(text.len());
    let tokens = tokenize(text, rules);

    // カーソルで終わる語(入力中の語)
    let word = tokens
        .iter()
        .find(|t| t.kind == TokenKind::Word && t.end == rel && is_word_text(t.text(text)));
    let (prefix, start) = word.map_or(("", rel), |t| (t.text(text), t.start));
    let ctx = context(&tokens, text, start);
    let tables = tables_in_statement(&tokens, text);

    let mut scored: Vec<(u8, Candidate)> = Vec::new();
    let mut push = |label: String, kind: CandidateKind, detail: Option<String>| {
        if let Some(score) = matches_prefix(&label, prefix) {
            // 完全に打ち終わっている語は、候補に出さない(すでにその語なので)
            if !label.eq_ignore_ascii_case(prefix) {
                scored.push((
                    score,
                    Candidate {
                        label,
                        kind,
                        detail,
                    },
                ));
            }
        }
    };
    match &ctx {
        Context::Table => {
            for t in &catalog.tables {
                push(t.name.clone(), CandidateKind::Table, Some(t.schema.clone()));
            }
            let mut schemas: Vec<&str> = catalog.tables.iter().map(|t| t.schema.as_str()).collect();
            schemas.sort_unstable();
            schemas.dedup();
            for s in schemas {
                push(s.to_string(), CandidateKind::Schema, None);
            }
        }
        Context::Qualified(q) => {
            // 別名、表名、スキーマ名の順に解決する
            let target = tables
                .iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(q))
                .map_or(q.as_str(), |(_, t)| t.as_str());
            for t in catalog
                .tables
                .iter()
                .filter(|t| t.name.eq_ignore_ascii_case(target))
            {
                for (c, ty) in &t.columns {
                    push(c.clone(), CandidateKind::Column, Some(ty.clone()));
                }
            }
            for t in catalog
                .tables
                .iter()
                .filter(|t| t.schema.eq_ignore_ascii_case(q))
            {
                push(t.name.clone(), CandidateKind::Table, Some(t.schema.clone()));
            }
        }
        Context::Expression => {
            for (_, name) in &tables {
                for t in catalog
                    .tables
                    .iter()
                    .filter(|t| t.name.eq_ignore_ascii_case(name))
                {
                    for (c, ty) in &t.columns {
                        push(c.clone(), CandidateKind::Column, Some(ty.clone()));
                    }
                }
            }
            if !prefix.is_empty() {
                for k in KEYWORDS {
                    push(keyword_case(k, prefix), CandidateKind::Keyword, None);
                }
            } else if tokens.iter().all(|t| t.kind.is_trivia()) {
                // 何も打っていない文の先頭では、文の開始語だけを出す(全部を並べても使えない)
                for k in STATEMENT_STARTERS {
                    push((*k).to_string(), CandidateKind::Keyword, None);
                }
            }
        }
    }
    // 前方一致を先に、次に種類(列、表、キーワードの順)、最後に名前で並べる
    scored.sort_by(|(sa, a), (sb, b)| {
        sa.cmp(sb)
            .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)).reverse())
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    scored.dedup_by(|(_, a), (_, b)| a.label == b.label && a.kind == b.kind);
    Suggestion {
        replace: base + start..base + rel,
        candidates: scored
            .into_iter()
            .take(MAX_CANDIDATES)
            .map(|(_, c)| c)
            .collect(),
    }
}
