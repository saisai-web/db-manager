//! 実際の MySQL / MariaDB に対する driver-mysql の結合テスト。
//! DB を用意できなければ飛ばす(`SQLTOOL_REQUIRE_MYSQL=1` なら失敗させる)。

macro_rules! my {
    () => {
        match test_mysql::database_or_skip() {
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
