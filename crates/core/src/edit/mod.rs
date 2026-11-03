//! テーブル編集。変更は DB に送らず、画面上の変更([`EditBuffer`])として保持する。
//! 保存するときに SQL をプレビューし([`plan`])、承認後に 1 つのトランザクションとして実行する。
//!
//! このモジュールは DB に依存しない純粋なロジックで、SQL の生成は `Dialect` が担う。

mod buffer;
mod change;
mod source;
mod sql;

pub use buffer::EditBuffer;
pub use change::{Change, ChangeKind, Value};
pub use source::{EditAccess, TableSource};
pub use sql::{EditError, PlannedEdit, plan};

#[cfg(test)]
mod sql_tests;
