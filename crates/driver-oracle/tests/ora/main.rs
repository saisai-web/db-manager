//! 実際の Oracle に対する driver-oracle の結合テスト。
//! DB を用意できなければ飛ばす(`SQLTOOL_REQUIRE_ORACLE=1` なら失敗させる)。
//! 接続先は環境変数(`SQLTOOL_TEST_ORACLE_HOST` など)で指定する。

macro_rules! ora {
    () => {
        match test_oracle::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

mod common;
mod control;
mod explain;
mod metadata;
mod readonly;
mod tls;
mod values;
