//! UI と同じ経路(コマンドを送り、イベントを受ける)で、実際の PostgreSQL に対して core を検証する。
//! DB を用意できなければ飛ばす(`SQLTOOL_REQUIRE_PG=1` なら失敗させる)。

/// 結果の DB を用意する。用意できなければ、そのテストを飛ばす。
macro_rules! pg {
    () => {
        match test_pg::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

mod cancel;
mod common;
mod connections;
mod editing;
mod execution;
mod explain;
mod history_schema;
mod io;
mod large_results;
mod lifecycle;
mod mysql;
mod oracle;
mod safety;
mod sqlite;
mod sqlserver;
mod ssh;
mod streaming;
