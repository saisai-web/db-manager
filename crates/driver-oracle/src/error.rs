use driver_api::{DriverError, ErrorKind};
use oracledb::Error;

const ORA_USER_REQUESTED_CANCEL: usize = 1013;
const ORA_INVALID_CREDENTIALS: usize = 1017;
const ORA_ACCOUNT_LOCKED: usize = 28000;
const ORA_PASSWORD_EXPIRED: usize = 28001;
const ORA_SESSION_KILLED: usize = 28;
const ORA_END_OF_FILE_ON_CHANNEL: usize = 3113;
const ORA_NOT_CONNECTED: usize = 3114;
const ORA_CONNECTION_LOST: usize = 3135;
const ORA_SESSION_TERMINATED: usize = 12537;

/// 接続の確立中のエラー。接続文字列やパスワードを含めない。
pub(crate) fn connect_error(e: &Error) -> DriverError {
    let mut err = map_error(e, "");
    if err.kind != ErrorKind::Auth {
        err.kind = ErrorKind::Connect;
    }
    err
}

/// `sql` は、エラーの位置を文字位置に直すための、実行した文。
pub(crate) fn map_error(e: &Error, sql: &str) -> DriverError {
    match e.kind() {
        oracledb::ErrorKind::DbError(db) => {
            let kind = match db.code() {
                ORA_INVALID_CREDENTIALS | ORA_ACCOUNT_LOCKED | ORA_PASSWORD_EXPIRED => {
                    ErrorKind::Auth
                }
                ORA_USER_REQUESTED_CANCEL => ErrorKind::Cancelled,
                ORA_SESSION_KILLED
                | ORA_END_OF_FILE_ON_CHANNEL
                | ORA_NOT_CONNECTED
                | ORA_CONNECTION_LOST
                | ORA_SESSION_TERMINATED => ErrorKind::Disconnected,
                _ => ErrorKind::Query,
            };
            let mut err = DriverError::new(kind, db.message().to_string());
            err.code = Some(format!("ORA-{:05}", db.code()));
            // 解析のエラーだけ、位置がある(実行時のエラーは 0 や無関係な値になる)
            err.position = (kind == ErrorKind::Query && is_parse_error(db.code()))
                .then(|| char_position(sql, db.offset()))
                .flatten();
            err
        }
        oracledb::ErrorKind::DeadConnection => {
            DriverError::new(ErrorKind::Disconnected, "DB との接続が切れました")
        }
        oracledb::ErrorKind::CallTimeoutExceeded => {
            DriverError::new(ErrorKind::Cancelled, "呼び出しの制限時間を超えました")
        }
        _ => DriverError::new(ErrorKind::Other, e.to_string()),
    }
}

/// SQL の構文や名前の解決で起きるエラー(文のどこかを指す)。
fn is_parse_error(code: usize) -> bool {
    matches!(code, 900..=999 | 1747 | 1756 | 1789 | 4043 | 6550)
}

/// Oracle が返す位置(文の先頭からのバイトの数)を、文字の位置にする。範囲外なら `None`。
fn char_position(sql: &str, offset: usize) -> Option<usize> {
    if sql.is_empty() || offset > sql.len() || !sql.is_char_boundary(offset) {
        return None;
    }
    Some(sql[..offset].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offsets_become_character_positions() {
        assert_eq!(char_position("SELECT 1 FORM dual", 14), Some(14));
        assert_eq!(char_position("日本語 x", 10), Some(4));
        assert_eq!(char_position("abc", 9), None);
    }
}
