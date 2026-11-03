//! 実際の PostgreSQL に対する driver-postgres の結合テスト。
//! DB を用意できなければ飛ばす(`SQLTOOL_REQUIRE_PG=1` なら失敗させる)。

macro_rules! pg {
    () => {
        match test_pg::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

mod common;
mod control;
mod metadata;
mod tls;
mod values;
