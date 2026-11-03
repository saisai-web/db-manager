//! SQL の字句解析と文分割。
//!
//! SQL を完全には解析せず、DB ごとの字句ルール([`Rules`])だけで文の境界と
//! 先頭キーワードを求める。未対応の構文があっても「分割に失敗して実行不能」に
//! ならないよう、判断できない場合は常に文を丸ごと 1 つとして扱う。

mod plsql;
mod scanner;
mod statement;

pub use scanner::{Rules, Token, TokenKind, tokenize};
pub use statement::{Statement, StatementKind, split, statement_at};
