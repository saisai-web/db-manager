//! 画面の統合テスト。ヘッドレスのウィンドウで、実際の PostgreSQL に接続した画面を操作する
//! (キー入力、クリック)。DB を用意できなければ飛ばす(`SQLTOOL_REQUIRE_PG=1` なら失敗させる)。

mod completion;
mod editing;
mod er;
mod harness;
mod io;
mod keys;
mod network_dbs;
mod query;
mod ssh;
