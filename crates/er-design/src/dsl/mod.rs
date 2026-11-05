//! 設計のテキスト記法。1 行 1 列で、表と関係を素早く書ける(読み書きは [`parse`] と [`print`])。
//!
//! ```text
//! @title ECサイト
//!
//! customers "顧客" "会員の基本情報"
//!   id     int  pk auto   "顧客ID"
//!   name   varchar(100) not null "氏名"
//!   email  varchar(255) unique
//!
//! orders "注文" @320,40
//!   id           bigint pk auto
//!   customer_id  int not null -> customers.id on delete cascade "顧客"
//!   index (customer_id)
//! ```

mod lex;
mod parse;
mod print;

use std::fmt;

pub use parse::parse;
pub use print::print;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1 始まりの行番号
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} 行目: {}", self.line, self.message)
    }
}

/// 名前として使える文字列か(空白・引用符・括弧・ドット・カンマを含まない)。
pub fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ".,()\"'`@\\".contains(c))
}

#[cfg(test)]
mod tests;
