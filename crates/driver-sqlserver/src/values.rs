//! 結果の値を、SQL Server が表示する形の文字列にする。数値や日時を f64 や日時型へ変換しない。

use std::fmt::Write as _;

use driver_api::ValueClass;
use tiberius::numeric::Numeric;
use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};
use tiberius::{Column, ColumnData, ColumnType};

pub(crate) fn classify(col: &Column) -> ValueClass {
    use ColumnType::*;
    match col.column_type() {
        Bit | Bitn => ValueClass::Bool,
        Int1 | Int2 | Int4 | Int8 | Intn | Float4 | Float8 | Floatn | Money | Money4 | Decimaln
        | Numericn => ValueClass::Number,
        Datetime4 | Datetime | Datetimen | Daten | Timen | Datetime2 | DatetimeOffsetn => {
            ValueClass::Temporal
        }
        BigVarBin | BigBinary | Image | Udt => ValueClass::Binary,
        Guid | BigVarChar | BigChar | NVarchar | NChar | Xml | Text | NText => ValueClass::Text,
        Null | SSVariant => ValueClass::Other,
    }
}

/// 画面に出す型名。メタデータだけでは幅が決まらない型(`Intn` など)は `None`。
pub(crate) fn type_name(col: &Column) -> Option<String> {
    use ColumnType::*;
    Some(
        match col.column_type() {
            Bit | Bitn => "bit",
            Int1 => "tinyint",
            Int2 => "smallint",
            Int4 => "int",
            Int8 => "bigint",
            Float4 => "real",
            Float8 => "float",
            Money => "money",
            Money4 => "smallmoney",
            Decimaln | Numericn => "decimal",
            Datetime | Datetimen => "datetime",
            Datetime4 => "smalldatetime",
            Daten => "date",
            Timen => "time",
            Datetime2 => "datetime2",
            DatetimeOffsetn => "datetimeoffset",
            Guid => "uniqueidentifier",
            BigVarBin => "varbinary",
            BigBinary => "binary",
            Image => "image",
            BigVarChar => "varchar",
            BigChar => "char",
            NVarchar => "nvarchar",
            NChar => "nchar",
            Text => "text",
            NText => "ntext",
            Xml => "xml",
            Udt => "udt",
            SSVariant => "sql_variant",
            Null | Intn | Floatn => return None,
        }
        .to_string(),
    )
}

/// 1 つのセルの値。`None` は NULL。バイナリは生のバイト列、それ以外は表示用の文字列(UTF-8)。
pub(crate) fn cell(data: &ColumnData<'_>) -> Option<Vec<u8>> {
    let text = match data {
        ColumnData::U8(v) => v.map(|x| x.to_string()),
        ColumnData::I16(v) => v.map(|x| x.to_string()),
        ColumnData::I32(v) => v.map(|x| x.to_string()),
        ColumnData::I64(v) => v.map(|x| x.to_string()),
        ColumnData::F32(v) => v.map(|x| x.to_string()),
        ColumnData::F64(v) => v.map(|x| x.to_string()),
        ColumnData::Bit(v) => v.map(|x| u8::from(x).to_string()),
        ColumnData::String(v) => v.as_ref().map(|s| s.to_string()),
        ColumnData::Guid(v) => v.map(|g| g.hyphenated().to_string().to_uppercase()),
        ColumnData::Binary(v) => return v.as_ref().map(|b| b.to_vec()),
        ColumnData::Numeric(v) => v.map(numeric),
        ColumnData::Xml(v) => v.as_ref().map(|x| x.to_string()),
        ColumnData::DateTime(v) => v.map(datetime),
        ColumnData::SmallDateTime(v) => v.map(small_datetime),
        ColumnData::Time(v) => v.map(time),
        ColumnData::Date(v) => v.map(date),
        ColumnData::DateTime2(v) => v.map(datetime2),
        ColumnData::DateTimeOffset(v) => v.map(datetimeoffset),
    };
    text.map(String::into_bytes)
}

/// 10 進数。小数点以下の桁数(スケール)を保つ(`1.50` は `1.50`)。
fn numeric(n: Numeric) -> String {
    let (value, scale) = (n.value(), n.scale() as usize);
    let digits = value.unsigned_abs().to_string();
    let sign = if value < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// 1 日目を 0001-01-01 とした日数から、(年, 月, 日)。先発グレゴリオ暦。
fn ymd(days_from_0001: i64) -> (i64, u32, u32) {
    // 0001-01-01 は、1970-01-01 の 719162 日前
    let z = days_from_0001 - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn date(d: Date) -> String {
    let (y, m, day) = ymd(i64::from(d.days()));
    format!("{y:04}-{m:02}-{day:02}")
}

/// 1900-01-01 からの日数(`datetime` と `smalldatetime`)。
fn date_1900(days: i64) -> String {
    let (y, m, d) = ymd(days + 693_595);
    format!("{y:04}-{m:02}-{d:02}")
}

fn datetime(v: DateTime) -> String {
    // 1 秒の 1/300 単位。ミリ秒は 3.33 ms 刻みで、SQL Server の表示と同じ丸めにする
    let frag = u64::from(v.seconds_fragments());
    let total_ms = (frag * 10 + 1) / 3; // = round(frag * 10 / 3)
    let (secs, ms) = (total_ms / 1000, total_ms % 1000);
    format!(
        "{} {:02}:{:02}:{:02}.{ms:03}",
        date_1900(i64::from(v.days())),
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn small_datetime(v: SmallDateTime) -> String {
    let minutes = u32::from(v.seconds_fragments());
    format!(
        "{} {:02}:{:02}:00",
        date_1900(i64::from(v.days())),
        minutes / 60,
        minutes % 60
    )
}

fn time(t: Time) -> String {
    let scale = u32::from(t.scale());
    let per_second = 10u64.pow(scale);
    let total = t.increments();
    let secs = total / per_second;
    let mut s = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    if scale > 0 {
        let _ = write!(s, ".{:0width$}", total % per_second, width = scale as usize);
    }
    s
}

fn datetime2(v: DateTime2) -> String {
    format!("{} {}", date(v.date()), time(v.time()))
}

/// 日時とオフセット。ワイヤ上の時刻は UTC なので、オフセットを足した現地時刻で表示する
/// (SQL Server の表示と同じ)。
fn datetimeoffset(v: DateTimeOffset) -> String {
    let off = i64::from(v.offset());
    let dt = v.datetime2();
    let scale = u32::from(dt.time().scale());
    let per_second = 10i64.pow(scale);
    let per_day = 86_400 * per_second;
    let total = i64::from(dt.date().days()) * per_day
        + dt.time().increments() as i64
        + off * 60 * per_second;
    let local = DateTime2::new(
        Date::new(total.div_euclid(per_day) as u32),
        Time::new(total.rem_euclid(per_day) as u64, scale as u8),
    );
    format!(
        "{} {}{:02}:{:02}",
        datetime2(local),
        if off < 0 { '-' } else { '+' },
        off.unsigned_abs() / 60,
        off.unsigned_abs() % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerics_keep_their_scale_and_sign() {
        assert_eq!(numeric(Numeric::new_with_scale(150, 2)), "1.50");
        assert_eq!(numeric(Numeric::new_with_scale(-5, 1)), "-0.5");
        assert_eq!(numeric(Numeric::new_with_scale(5, 3)), "0.005");
        assert_eq!(numeric(Numeric::new_with_scale(42, 0)), "42");
        assert_eq!(
            numeric(Numeric::new_with_scale(
                12345678901234567890123456789012345678,
                18
            )),
            "12345678901234567890.123456789012345678"
        );
    }

    #[test]
    fn dates_are_formatted_in_the_proleptic_gregorian_calendar() {
        assert_eq!(date(Date::new(0)), "0001-01-01");
        assert_eq!(date(Date::new(739_921)), "2026-11-02");
        assert_eq!(date(Date::new(738_944)), "2024-02-29");
        assert_eq!(date_1900(0), "1900-01-01");
        assert_eq!(date_1900(45_961), "2025-11-02");
    }

    #[test]
    fn times_keep_their_fractional_digits() {
        assert_eq!(time(Time::new(37_230_500, 3)), "10:20:30.500");
        assert_eq!(time(Time::new(45_296, 0)), "12:34:56");
        assert_eq!(time(Time::new(123_456_789_000, 7)), "03:25:45.6789000");
    }

    #[test]
    fn datetime_rounds_to_the_servers_milliseconds() {
        // 12:34:56.123 は 45296.123 秒 = 13588837 / 300 秒
        assert_eq!(
            datetime(DateTime::new(0, 13_588_837)),
            "1900-01-01 12:34:56.123"
        );
        assert_eq!(
            small_datetime(SmallDateTime::new(1, 754)),
            "1900-01-02 12:34:00"
        );
    }

    #[test]
    fn offsets_are_signed() {
        // ワイヤ上は UTC。+09:00 なら 9 時間進め、日をまたぐ
        let dt = DateTime2::new(Date::new(739_921), Time::new(0, 0));
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(dt, 540)),
            "2026-11-02 09:00:00 +09:00"
        );
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(dt, -90)),
            "2026-11-01 22:30:00 -01:30"
        );
    }
}
