//! 結果の保持と一時ファイルへの退避。
//!
//! 値は DB が返した文字列(またはバイト列)のまま、チャンク単位で連続したバイト列に詰める。
//! メモリ上限を超えたら、最も長く参照されていないチャンクを一時ファイルへ退避する。
//!
//! # スレッド
//!
//! * 受信側(Tokio タスクなど)が [`ResultStore::push_row`] で追記する。
//! * UI スレッドは [`ResultStore::with_row`] で表示範囲の行だけを読む。この呼び出しは
//!   ファイル I/O を行わない。退避済みの行は専用スレッドが読み戻し、完了までは
//!   [`Access::Loading`] を返す。読み戻しの完了は [`ResultStore::set_on_ready`] で知らせる。
//! * 一時ファイルの読み書きは、専用スレッド(最初の退避で起動)が担当する。

mod chunk;
mod engine;
mod sort;
mod spill;
mod store;
mod types;

pub use chunk::RowView;
pub use sort::{SortIndex, SortKind, SortOrder};
pub use store::ResultStore;
pub use types::{Access, Pressure, Stats, StoreConfig, StoreError};
