//! 結果の列の分類と、値の文字列化。値は、Oracle が返した内容のまま、正確な文字列にする
//! (NUMBER を f64 にしない。日時は秒の小数まで落とさない)。

use driver_api::{ResultColumn, ValueClass};
use oracledb::{Metadata, OracleIntervalDS, OracleIntervalYM, OracleNumber, OracleTimestamp, Row};

/// 列の型ごとの取り出し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Number,
    Text,
    Binary,
    BinaryDouble,
    BinaryFloat,
    /// DATE。秒まで
    Date,
    /// TIMESTAMP。小数の桁数
    Timestamp(u8),
    /// TIMESTAMP WITH TIME ZONE。ライブラリは UTC に直して返す(元のオフセットは分からない)
    TimestampTz(u8),
    IntervalDs,
    IntervalYm,
    Bool,
    /// JSON、VECTOR、オブジェクトなど。値の代わりに、型名を示す
    Unsupported,
}

pub(crate) fn kind(m: &Metadata) -> Kind {
    match m.db_type().name() {
        "DB_TYPE_NUMBER" | "DB_TYPE_BINARY_INTEGER" => Kind::Number,
        "DB_TYPE_BINARY_DOUBLE" => Kind::BinaryDouble,
        "DB_TYPE_BINARY_FLOAT" => Kind::BinaryFloat,
        "DB_TYPE_CHAR"
        | "DB_TYPE_NCHAR"
        | "DB_TYPE_VARCHAR"
        | "DB_TYPE_NVARCHAR"
        | "DB_TYPE_LONG"
        | "DB_TYPE_LONG_NVARCHAR"
        | "DB_TYPE_CLOB"
        | "DB_TYPE_NCLOB"
        | "DB_TYPE_ROWID"
        | "DB_TYPE_UROWID" => Kind::Text,
        "DB_TYPE_RAW" | "DB_TYPE_LONG_RAW" | "DB_TYPE_BLOB" => Kind::Binary,
        "DB_TYPE_DATE" => Kind::Date,
        "DB_TYPE_TIMESTAMP" | "DB_TYPE_TIMESTAMP_LTZ" => Kind::Timestamp(m.scale().max(0) as u8),
        "DB_TYPE_TIMESTAMP_TZ" => Kind::TimestampTz(m.scale().max(0) as u8),
        "DB_TYPE_INTERVAL_DS" => Kind::IntervalDs,
        "DB_TYPE_INTERVAL_YM" => Kind::IntervalYm,
        "DB_TYPE_BOOLEAN" => Kind::Bool,
        _ => Kind::Unsupported,
    }
}

fn class(kind: Kind) -> ValueClass {
    match kind {
        Kind::Number | Kind::BinaryDouble | Kind::BinaryFloat => ValueClass::Number,
        Kind::Text => ValueClass::Text,
        Kind::Binary => ValueClass::Binary,
        Kind::Date
        | Kind::Timestamp(_)
        | Kind::TimestampTz(_)
        | Kind::IntervalDs
        | Kind::IntervalYm => ValueClass::Temporal,
        Kind::Bool => ValueClass::Bool,
        Kind::Unsupported => ValueClass::Other,
    }
}

pub(crate) fn column(m: &Metadata) -> ResultColumn {
    ResultColumn {
        name: m.name().to_string(),
        type_name: Some(m.data_type()),
        class: class(kind(m)),
    }
}

/// 1 つのセル。`None` が NULL(Oracle は空文字列も NULL になる)。
pub(crate) fn cell(
    row: &Row,
    i: usize,
    kind: Kind,
    type_name: &str,
) -> Result<Option<Vec<u8>>, oracledb::Error> {
    Ok(match kind {
        Kind::Number => row
            .get::<Option<OracleNumber>>(i)?
            .map(|n| n.to_string().into_bytes()),
        Kind::Text => row.get::<Option<String>>(i)?.map(String::into_bytes),
        Kind::Binary => row.get::<Option<Vec<u8>>>(i)?,
        Kind::BinaryDouble => row
            .get::<Option<f64>>(i)?
            .map(|v| float_text(v).into_bytes()),
        Kind::BinaryFloat => row
            .get::<Option<f32>>(i)?
            .map(|v| float_text(f64::from(v)).into_bytes()),
        Kind::Date => row
            .get::<Option<OracleTimestamp>>(i)?
            .map(|t| timestamp_text(&t, 0, false).into_bytes()),
        Kind::Timestamp(scale) => row
            .get::<Option<OracleTimestamp>>(i)?
            .map(|t| timestamp_text(&t, scale, false).into_bytes()),
        Kind::TimestampTz(scale) => row
            .get::<Option<OracleTimestamp>>(i)?
            .map(|t| timestamp_text(&t, scale, true).into_bytes()),
        Kind::IntervalDs => row
            .get::<Option<OracleIntervalDS>>(i)?
            .map(|v| interval_ds_text(&v).into_bytes()),
        Kind::IntervalYm => row
            .get::<Option<OracleIntervalYM>>(i)?
            .map(|v| interval_ym_text(&v).into_bytes()),
        Kind::Bool => row
            .get::<Option<bool>>(i)?
            .map(|b| if b { "TRUE" } else { "FALSE" }.as_bytes().to_vec()),
        // NULL かどうかは確かめられるので、値があるときだけ、型名を示す
        Kind::Unsupported => match row.get::<Option<String>>(i) {
            Ok(None) => None,
            _ => Some(format!("(未対応の型: {type_name})").into_bytes()),
        },
    })
}

/// 浮動小数点。往復できる最短の表記。非数と無限大は、Oracle の書き方に合わせる。
fn float_text(v: f64) -> String {
    if v.is_nan() {
        "Nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Inf" } else { "-Inf" }.into()
    } else {
        v.to_string()
    }
}

/// `YYYY-MM-DD HH24:MI:SS[.小数]`。TIMESTAMP WITH TIME ZONE は UTC に直された値なので、`UTC` を付ける。
fn timestamp_text(t: &OracleTimestamp, scale: u8, utc_label: bool) -> String {
    let mut s = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    let digits = usize::from(scale.min(9));
    if digits > 0 {
        s.push_str(&format!(".{:09}", t.nanoseconds())[..=digits]);
    }
    if utc_label && t.is_utc() {
        s.push_str(" UTC");
    }
    s
}

/// `+DD HH:MM:SS.fffffffff`(Oracle の表記)。
fn interval_ds_text(v: &OracleIntervalDS) -> String {
    let negative =
        v.days() < 0 || v.hours() < 0 || v.minutes() < 0 || v.seconds() < 0 || v.nanoseconds() < 0;
    format!(
        "{}{:02} {:02}:{:02}:{:02}.{:09}",
        if negative { '-' } else { '+' },
        v.days().unsigned_abs(),
        v.hours().unsigned_abs(),
        v.minutes().unsigned_abs(),
        v.seconds().unsigned_abs(),
        v.nanoseconds().unsigned_abs()
    )
}

/// `+YY-MM`(Oracle の表記)。
fn interval_ym_text(v: &OracleIntervalYM) -> String {
    let negative = v.years() < 0 || v.months() < 0;
    format!(
        "{}{:02}-{:02}",
        if negative { '-' } else { '+' },
        v.years().unsigned_abs(),
        v.months().unsigned_abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_use_oracles_spelling_for_special_values() {
        assert_eq!(float_text(1.5), "1.5");
        assert_eq!(float_text(0.1), "0.1");
        assert_eq!(float_text(f64::NAN), "Nan");
        assert_eq!(float_text(f64::INFINITY), "Inf");
        assert_eq!(float_text(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn timestamps_keep_the_columns_fraction_digits() {
        let t = OracleTimestamp::new_timestamp(2026, 11, 2, 12, 34, 56, 123_456_789);
        assert_eq!(timestamp_text(&t, 0, false), "2026-11-02 12:34:56");
        assert_eq!(timestamp_text(&t, 6, false), "2026-11-02 12:34:56.123456");
        assert_eq!(
            timestamp_text(&t, 9, false),
            "2026-11-02 12:34:56.123456789"
        );
        let u = OracleTimestamp::new_utc_timestamp(2026, 11, 2, 3, 34, 56, 500_000_000);
        assert_eq!(
            timestamp_text(&u, 6, true),
            "2026-11-02 03:34:56.500000 UTC"
        );
    }

    #[test]
    fn intervals_use_oracles_spelling() {
        assert_eq!(interval_ym_text(&OracleIntervalYM::new(1, 2)), "+01-02");
        assert_eq!(interval_ym_text(&OracleIntervalYM::new(-1, -2)), "-01-02");
        assert_eq!(
            interval_ds_text(&OracleIntervalDS::new(1, 2, 3, 4, 500_000_000)),
            "+01 02:03:04.500000000"
        );
    }
}
