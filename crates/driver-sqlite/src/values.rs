//! SQLite の値の文字列化と、列の分類。
//!
//! SQLite は値に型を持つが、列には型の「親和性」しかない。値は DB の表記のまま文字列にし、
//! f64 などへ変換して桁を落とすことはしない(整数は 10 進数、実数は最短の往復可能な表記)。

use driver_api::ValueClass;
use rusqlite::types::ValueRef;

/// 値をバイト列にする。`None` が NULL。BLOB は、そのままのバイト列。
pub(crate) fn cell(v: ValueRef<'_>) -> Option<Vec<u8>> {
    match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string().into_bytes()),
        ValueRef::Real(f) => Some(real_text(f).into_bytes()),
        ValueRef::Text(t) => Some(t.to_vec()),
        ValueRef::Blob(b) => Some(b.to_vec()),
    }
}

fn real_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}") // 整数値の実数は、実数だと分かるように「1.0」と書く
    } else {
        format!("{f}")
    }
}

/// 実際の値の型から、分類する(宣言された型がない列に使う)。NULL からは分からない。
pub(crate) fn value_class(v: ValueRef<'_>) -> ValueClass {
    match v {
        ValueRef::Integer(_) | ValueRef::Real(_) => ValueClass::Number,
        ValueRef::Text(_) => ValueClass::Text,
        ValueRef::Blob(_) => ValueClass::Binary,
        ValueRef::Null => ValueClass::Other,
    }
}

/// 宣言された型から、列の分類を決める(SQLite の型親和性の規則に従う)。
/// 宣言がない列(式の結果など)は、値の型が行ごとに違いうるため、分類しない。
pub(crate) fn classify(decl: Option<&str>) -> ValueClass {
    let Some(t) = decl.map(str::to_ascii_uppercase) else {
        return ValueClass::Other;
    };
    if t.contains("BOOL") {
        ValueClass::Bool
    } else if t.contains("INT")
        || t.contains("REAL")
        || t.contains("FLOA")
        || t.contains("DOUB")
        || t.contains("NUMERIC")
        || t.contains("DECIMAL")
    {
        ValueClass::Number
    } else if t.contains("BLOB") {
        ValueClass::Binary
    } else if t.contains("DATE") || t.contains("TIME") {
        ValueClass::Temporal
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        ValueClass::Text
    } else {
        ValueClass::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_exact_and_reals_round_trip() {
        assert_eq!(
            cell(ValueRef::Integer(i64::MAX)).unwrap(),
            b"9223372036854775807"
        );
        assert_eq!(cell(ValueRef::Real(0.1)).unwrap(), b"0.1");
        assert_eq!(cell(ValueRef::Real(3.0)).unwrap(), b"3.0");
        assert_eq!(
            cell(ValueRef::Real(1e21)).unwrap(),
            b"1000000000000000000000"
        );
        assert_eq!(cell(ValueRef::Real(f64::INFINITY)).unwrap(), b"Infinity");
        assert!(cell(ValueRef::Null).is_none());
        assert_eq!(
            cell(ValueRef::Text("日本語".as_bytes())).unwrap(),
            "日本語".as_bytes()
        );
        assert_eq!(cell(ValueRef::Blob(&[0, 255])).unwrap(), [0, 255]);
    }

    #[test]
    fn declared_types_map_by_affinity_rules() {
        use ValueClass::*;
        for (decl, want) in [
            (Some("INTEGER"), Number),
            (Some("BIGINT"), Number),
            (Some("REAL"), Number),
            (Some("DOUBLE PRECISION"), Number),
            (Some("NUMERIC(10,2)"), Number),
            (Some("DECIMAL"), Number),
            (Some("TEXT"), Text),
            (Some("VARCHAR(20)"), Text),
            (Some("BLOB"), Binary),
            (Some("BOOLEAN"), Bool),
            (Some("DATETIME"), Temporal),
            (Some("TIMESTAMP"), Temporal),
            (Some("JSON"), Other),
            (None, Other),
        ] {
            assert_eq!(classify(decl), want, "{decl:?}");
        }
    }
}
