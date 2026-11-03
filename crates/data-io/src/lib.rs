//! CSV / Excel の書き出しと、CSV の読み取り。DB に依存しない。
//!
//! 値は DB が返した文字列のまま扱い、数値を f64 に変換して桁を落とすことはしない
//! (Excel のセルが数値になるのは、15 桁以内で、桁落ちしない場合だけ)。

mod csv_read;
mod csv_write;
mod encoding;
mod error;
mod xlsx;

pub use csv_read::{CsvData, CsvReader, ReadOptions};
pub use csv_write::{CsvOptions, CsvWriter};
pub use encoding::{TextEncoding, decode, detect};
pub use error::IoError;
pub use xlsx::{CellKind, XlsxColumn, write_xlsx};
