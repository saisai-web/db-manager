use driver_api::{DriverError, ErrorKind};
use tokio_postgres::error::{ErrorPosition, SqlState};

/// 接続の確立中のエラー。接続文字列やパスワードを含めない。
pub(crate) fn connect_error(e: tokio_postgres::Error) -> DriverError {
    let mut err = map_error(&e);
    if err.kind == ErrorKind::Query {
        err.kind = match e.code() {
            Some(c) if is_auth_failure(c) => ErrorKind::Auth,
            _ => ErrorKind::Connect,
        };
    } else if err.kind != ErrorKind::Auth {
        err.kind = ErrorKind::Connect;
    }
    err
}

fn is_auth_failure(code: &SqlState) -> bool {
    *code == SqlState::INVALID_PASSWORD || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
}

pub(crate) fn map_error(e: &tokio_postgres::Error) -> DriverError {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        let kind = if *code == SqlState::QUERY_CANCELED {
            ErrorKind::Cancelled
        } else if is_auth_failure(code) {
            ErrorKind::Auth
        } else if code.code().starts_with("08") || *code == SqlState::ADMIN_SHUTDOWN {
            ErrorKind::Disconnected
        } else {
            ErrorKind::Query
        };
        let mut err = DriverError::new(kind, db.message());
        err.code = Some(code.code().to_string());
        err.detail = db.detail().map(str::to_string);
        err.hint = db.hint().map(str::to_string);
        err.position = match db.position() {
            // 1 始まりの文字位置を 0 始まりにする
            Some(ErrorPosition::Original(p)) => (*p as usize).checked_sub(1),
            _ => None,
        };
        return err;
    }
    if e.is_closed() {
        return DriverError::new(ErrorKind::Disconnected, "DB との接続が切れました");
    }
    // 非 DB エラー(I/O、プロトコル)。原因の連鎖を 1 行にまとめる
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    DriverError::new(ErrorKind::Other, msg)
}
