use driver_api::{DriverError, ErrorKind};
use rusqlite::ErrorCode;

/// 先頭から `byte` バイト目の位置を、文字位置にする。
fn char_position(sql: &str, byte: usize) -> usize {
    sql.get(..byte).map_or(byte, |head| head.chars().count())
}

pub(crate) fn map_error(e: rusqlite::Error, sql: &str) -> DriverError {
    let (code, msg, position) = match &e {
        rusqlite::Error::SqliteFailure(f, m) => (
            Some(f.code),
            m.clone().unwrap_or_else(|| e.to_string()),
            None,
        ),
        rusqlite::Error::SqlInputError {
            error, msg, offset, ..
        } => (
            Some(error.code),
            msg.clone(),
            usize::try_from(*offset).ok().map(|o| char_position(sql, o)),
        ),
        other => (None, other.to_string(), None),
    };
    let kind = match code {
        Some(ErrorCode::OperationInterrupted) => ErrorKind::Cancelled,
        Some(ErrorCode::CannotOpen | ErrorCode::NotADatabase | ErrorCode::PermissionDenied) => {
            ErrorKind::Connect
        }
        _ => ErrorKind::Query,
    };
    let mut err = DriverError::new(kind, msg);
    err.code = code.map(|c| format!("{c:?}"));
    err.position = position;
    err
}
