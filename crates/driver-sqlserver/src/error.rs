use driver_api::{DriverError, ErrorKind};
use tiberius::error::Error;

const LOGIN_FAILED: u32 = 18456;
const LOGIN_FAILED_DB: u32 = 4060;
const SYNTAX_ERROR: u32 = 102;
const SERVER_SHUTDOWN: u32 = 6005;

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
            let kind = match s.code() {
                LOGIN_FAILED => ErrorKind::Auth,
                LOGIN_FAILED_DB => ErrorKind::Connect,
                SERVER_SHUTDOWN => ErrorKind::Disconnected,
                _ => ErrorKind::Query,
            };
            let mut err = DriverError::new(kind, s.message().to_string());
            err.code = Some(s.code().to_string());
            err
        }
        Error::Io { .. } => DriverError::new(ErrorKind::Disconnected, "DB との接続が切れました"),
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

/// 実行した文のエラー。`Incorrect syntax near 'X'` の `X` から、文の中の位置を求める。
pub(crate) fn query_error(e: &Error, sql: &str) -> DriverError {
    let mut err = map_error(e);
    if let Error::Server(s) = e
        && s.code() == SYNTAX_ERROR
    {
        err.position = locate(s.message(), s.line(), sql);
    }
    err
}

/// SQL Server は、エラーの箇所の語を `near '...'` で示し、バッチ内の行番号(1 始まり)を返す。
/// その行から語を探し、文の先頭からの文字位置にする。
fn locate(message: &str, line: u32, sql: &str) -> Option<usize> {
    let (_, rest) = message.split_once("near '")?;
    let (token, _) = rest.rsplit_once('\'')?;
    let line_start: usize = sql
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    let byte = line_start + sql[line_start..].find(token)?;
    Some(sql[..byte].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_errors_are_located_by_the_quoted_token() {
        let m = "Incorrect syntax near 'FORM'.";
        assert_eq!(locate(m, 1, "SELECT * FORM t"), Some(9));
        // 2 行目の語を探す(1 行目に同じ語があっても、その行から探す)
        assert_eq!(locate(m, 2, "FORM x\nSELECT * FORM t"), Some(7 + 9));
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        let m = "Incorrect syntax near 'FORM'.";
        assert_eq!(locate(m, 1, "SELECT N'日本' FORM t"), Some(13));
    }

    #[test]
    fn messages_without_a_token_get_no_position() {
        assert_eq!(
            locate("Invalid object name 't'.", 1, "select * from t"),
            None
        );
    }
}
