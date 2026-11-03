//! トランザクションの状態と、読み取り専用モード。
//!
//! Oracle は、最初の文で暗黙にトランザクションが始まり、COMMIT まで続く。他の DB と同じく、
//! 1 文ずつ確定する(自動コミット)。UI の未コミットの検知、タブを閉じたときの変更の消失、
//! ロックの残りを避けるため。明示的に始めたトランザクション(`SAVEPOINT sqltool_begin` や
//! `SET TRANSACTION`)の間だけ、確定しない。
//!
//! 読み取り専用モードは、セッション全体の設定がないので、トランザクションで実現する。
//! 明示的なトランザクションの外では、文ごとに `SET TRANSACTION READ ONLY` で始め、文が終わったら
//! 必ず ROLLBACK する。書き込みは、サーバーが ORA-01456 で拒否する。利用者の
//! `begin_sql` は、読み取り専用のトランザクションとして始まる。

use std::sync::atomic::{AtomicBool, Ordering};

use driver_api::{DriverError, ErrorKind};

/// セッションと、ワーカーのスレッドで共有する状態。
#[derive(Default)]
pub(crate) struct Mode {
    pub read_only: AtomicBool,
    /// ワーカーが、明示的なトランザクションの中にいる(実行計画が、トランザクションの外かを知るため)
    pub in_transaction: AtomicBool,
}

impl Mode {
    pub fn read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    pub fn in_transaction(&self) -> bool {
        self.in_transaction.load(Ordering::SeqCst)
    }
}

/// トランザクションの状態。Oracle は、最初の文で暗黙にトランザクションが始まり、COMMIT まで続く。
/// 他の DB と同じく、1 文ずつ確定する(自動コミット)。UI の未コミットの検知、タブを閉じたときの
/// 変更の消失、ロックの残りを避けるため。明示的に始めたトランザクション(`SAVEPOINT sqltool_begin`
/// や `SET TRANSACTION`)の間だけ、確定しない。
#[derive(Default)]
pub(crate) struct Txn {
    pub explicit: bool,
}

impl Txn {
    /// 文を実行した後の、状態の更新。
    pub fn after(&mut self, sql: &str, ddl: bool) {
        let head: Vec<String> = sql
            .split_whitespace()
            .take(3)
            .map(str::to_ascii_uppercase)
            .collect();
        let word = |i: usize| head.get(i).map_or("", String::as_str);
        match (word(0), word(1)) {
            ("SAVEPOINT", "SQLTOOL_BEGIN") | ("SET", "TRANSACTION") => self.explicit = true,
            // `ROLLBACK TO SAVEPOINT` は、トランザクションを終わらせない
            ("ROLLBACK", "TO") => {}
            ("COMMIT", _) | ("ROLLBACK", _) => self.explicit = false,
            _ if ddl => self.explicit = false, // DDL は、暗黙にコミットする
            _ => {}
        }
    }
}

/// 読み取り専用モードで、利用者が実行しようとした `SET TRANSACTION` が、読み取り専用のものか。
/// `Ok(false)` は `SET TRANSACTION` ではない文。`READ WRITE` や `ISOLATION LEVEL` のものは、
/// 読み取り専用が外れるので拒否する。
pub(crate) fn check_set_transaction(sql: &str) -> Result<bool, DriverError> {
    let words: Vec<String> = sql
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    let is_set = |i: usize, w: &str| words.get(i).is_some_and(|x| x == w);
    if !(is_set(0, "SET") && is_set(1, "TRANSACTION")) {
        return Ok(false);
    }
    let read_only = words.windows(2).any(|w| w[0] == "READ" && w[1] == "ONLY");
    if read_only {
        Ok(true)
    } else {
        Err(DriverError::new(
            ErrorKind::Query,
            "読み取り専用モードでは、SET TRANSACTION は READ ONLY のものだけ実行できます",
        ))
    }
}

/// `ALTER SESSION`。DDL に分類されるが、トランザクションを終わらせず、データも変えない。
pub(crate) fn is_alter_session(sql: &str) -> bool {
    let mut words = sql.split_whitespace().map(str::to_ascii_uppercase);
    words.next().as_deref() == Some("ALTER") && words.next().as_deref() == Some("SESSION")
}

/// 読み取り専用モードの文の前に、トランザクションを読み取り専用で始める必要があるか。
/// 明示的なトランザクションの中(すでに読み取り専用で始まっている)と、利用者自身の
/// `SET TRANSACTION READ ONLY` では要らない。
pub(crate) fn needs_read_only_begin(txn: &Txn, sql: &str) -> Result<bool, DriverError> {
    Ok(!check_set_transaction(sql)? && !txn.explicit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_transactions_are_tracked_from_the_statements() {
        let mut t = Txn::default();
        t.after("SAVEPOINT sqltool_begin", false);
        assert!(t.explicit);
        t.after("rollback to savepoint x", false);
        assert!(t.explicit);
        t.after("COMMIT", false);
        assert!(!t.explicit);
        t.after("set transaction read only", false);
        assert!(t.explicit);
        t.after("ROLLBACK", false);
        assert!(!t.explicit);
        t.after("SET TRANSACTION READ WRITE", false);
        t.after("CREATE TABLE x (a int)", true);
        assert!(!t.explicit, "DDL commits implicitly");
        t.after("INSERT INTO x VALUES (1)", false);
        assert!(!t.explicit);
    }

    #[test]
    fn alter_session_is_told_apart_from_other_alters() {
        assert!(is_alter_session("alter  session set nls_language = 'X'"));
        assert!(!is_alter_session("ALTER TABLE t ADD (a int)"));
        assert!(!is_alter_session("ALTER SYSTEM KILL SESSION '1,2'"));
    }

    #[test]
    fn only_read_only_transactions_may_be_started_by_hand() {
        assert!(!check_set_transaction("select 1 from dual").unwrap());
        assert!(check_set_transaction("set transaction read only").unwrap());
        assert!(check_set_transaction("SET TRANSACTION  READ\n ONLY NAME 'x'").unwrap());
        assert!(check_set_transaction("SET TRANSACTION READ WRITE").is_err());
        assert!(check_set_transaction("set transaction isolation level serializable").is_err());
    }
}
