use driver_api::{DriverError, ErrorKind};
use mysql_async::Error;

const ER_ACCESS_DENIED: u16 = 1045;
const ER_DBACCESS_DENIED: u16 = 1044;
const ER_ACCESS_DENIED_NO_PASSWORD: u16 = 1698;
const ER_QUERY_INTERRUPTED: u16 = 1317;
const ER_MAX_EXECUTION_TIME: u16 = 3024;
const ER_SERVER_SHUTDOWN: u16 = 1053;
const ER_CONNECTION_KILLED: u16 = 1927;
const CR_SERVER_GONE: u16 = 2006;
const CR_SERVER_LOST: u16 = 2013;

/// 接続の確立中のエラー。接続文字列やパスワードを含めない。
pub(crate) fn connect_error(e: &Error) -> DriverError {
    let mut err = map_error(e);
    if err.kind != ErrorKind::Auth {
        err.kind = ErrorKind::Connect;
    }
    err
}

pub(crate) fn map_error(e: &Error) -> DriverError {
    match e {
        Error::Server(s) => {
            let kind = match s.code {
                ER_ACCESS_DENIED | ER_DBACCESS_DENIED | ER_ACCESS_DENIED_NO_PASSWORD => {
                    ErrorKind::Auth
                }
                ER_QUERY_INTERRUPTED | ER_MAX_EXECUTION_TIME => ErrorKind::Cancelled,
                ER_SERVER_SHUTDOWN | ER_CONNECTION_KILLED | CR_SERVER_GONE | CR_SERVER_LOST => {
                    ErrorKind::Disconnected
                }
                _ => ErrorKind::Query,
            };
            let mut err = DriverError::new(kind, s.message.clone());
            err.code = Some(if s.state.is_empty() || s.state == "HY000" {
                s.code.to_string()
            } else {
                format!("{} {}", s.code, s.state)
            });
            err
        }
        Error::Io(_) => DriverError::new(ErrorKind::Disconnected, "DB との接続が切れました"),
        other => DriverError::new(ErrorKind::Other, one_line(other)),
    }
}

/// 原因の連鎖を 1 行にまとめる。
fn one_line(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

/// 構文エラーの `near '...'` から、文の中の位置(0 始まりの文字位置)を求める。
/// MySQL は位置を直接返さず、エラーの箇所の続きを引用するだけのため、近似になる。
pub(crate) fn locate(err: &mut DriverError, sql: &str) {
    if err.kind != ErrorKind::Query || err.position.is_some() {
        return;
    }
    let Some(rest) = err.message.split_once("near '").map(|(_, r)| r) else {
        return;
    };
    let Some((quoted, _)) = rest.rsplit_once("' at line") else {
        return;
    };
    // 引用されるのは、エラーの箇所から文の終わりまで(長いときは途中まで)
    let byte = if quoted.is_empty() {
        sql.len()
    } else if sql.ends_with(quoted) {
        sql.len() - quoted.len()
    } else {
        match sql.find(quoted) {
            Some(b) => b,
            None => return,
        }
    };
    err.position = Some(sql[..byte].chars().count());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax_error(near: &str) -> DriverError {
        DriverError::new(
            ErrorKind::Query,
            format!(
                "You have an error in your SQL syntax; check the manual for the right syntax to use near '{near}' at line 1"
            ),
        )
    }

    #[test]
    fn syntax_errors_are_located_by_the_quoted_text() {
        let mut e = syntax_error("FORM t");
        locate(&mut e, "select * FORM t");
        assert_eq!(e.position, Some(9));
        let mut e = syntax_error("");
        locate(&mut e, "select * from");
        assert_eq!(e.position, Some(13));
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        let mut e = syntax_error("FORM t");
        locate(&mut e, "select '日本' FORM t");
        assert_eq!(e.position, Some(12));
    }

    #[test]
    fn other_errors_get_no_position() {
        let mut e = DriverError::new(ErrorKind::Query, "Table 'x' doesn't exist");
        locate(&mut e, "select * from x");
        assert_eq!(e.position, None);
    }
}
