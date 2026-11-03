//! 実際の SQL Server に対する driver-sqlserver の結合テスト。
//! 接続先は `SQLTOOL_TEST_MSSQL_HOST` などで指定する。無ければ飛ばす(`SQLTOOL_REQUIRE_MSSQL=1` なら失敗させる)。

macro_rules! ms {
    () => {
        match test_sqlserver::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

mod common;
mod control;
mod metadata;
mod plan;
mod readonly;
mod tls;
mod values;
