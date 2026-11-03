use driver_api::ValueClass;
use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::{Column, Value};

/// `binary` の文字セット番号。BLOB / BINARY / VARBINARY の列がこの値になる。
const CHARSET_BINARY: u16 = 63;

pub(crate) fn classify(col: &Column) -> ValueClass {
    use ColumnType::*;
    let binary_charset = col.character_set() == CHARSET_BINARY;
    match col.column_type() {
        MYSQL_TYPE_DECIMAL
        | MYSQL_TYPE_NEWDECIMAL
        | MYSQL_TYPE_TINY
        | MYSQL_TYPE_SHORT
        | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_INT24
        | MYSQL_TYPE_YEAR
        | MYSQL_TYPE_FLOAT
        | MYSQL_TYPE_DOUBLE => ValueClass::Number,
        MYSQL_TYPE_DATE
        | MYSQL_TYPE_NEWDATE
        | MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2
        | MYSQL_TYPE_TIME
        | MYSQL_TYPE_TIME2 => ValueClass::Temporal,
        MYSQL_TYPE_BIT | MYSQL_TYPE_GEOMETRY => ValueClass::Binary,
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING
            if binary_charset && !col.flags().contains(ColumnFlags::ENUM_FLAG) =>
        {
            ValueClass::Binary
        }
        MYSQL_TYPE_JSON
        | MYSQL_TYPE_ENUM
        | MYSQL_TYPE_SET
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING
        | MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB => ValueClass::Text,
        _ => ValueClass::Other,
    }
}

/// 画面に出す型名。
pub(crate) fn type_name(col: &Column) -> Option<String> {
    use ColumnType::*;
    let unsigned = col.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    let binary = col.character_set() == CHARSET_BINARY;
    let name = match col.column_type() {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_INT24 => "mediumint",
        MYSQL_TYPE_LONG => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_YEAR => "year",
        MYSQL_TYPE_BIT => "bit",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_ENUM => "enum",
        MYSQL_TYPE_SET => "set",
        MYSQL_TYPE_GEOMETRY => "geometry",
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB => {
            if binary {
                "blob"
            } else {
                "text"
            }
        }
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => {
            if binary {
                "varbinary"
            } else {
                "varchar"
            }
        }
        MYSQL_TYPE_STRING => {
            if col.flags().contains(ColumnFlags::ENUM_FLAG) {
                "enum"
            } else if col.flags().contains(ColumnFlags::SET_FLAG) {
                "set"
            } else if binary {
                "binary"
            } else {
                "char"
            }
        }
        _ => return None,
    };
    let numeric = matches!(
        col.column_type(),
        MYSQL_TYPE_TINY
            | MYSQL_TYPE_SHORT
            | MYSQL_TYPE_INT24
            | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG
    );
    Some(if unsigned && numeric {
        format!("{name} unsigned")
    } else {
        name.to_string()
    })
}

/// バインド付きの実行(バイナリプロトコル)で返った値の文字列表現。
/// 結果を画面に出す経路(テキストプロトコル)では使わない。
pub(crate) fn value_text(v: &Value) -> Option<Vec<u8>> {
    Some(match v {
        Value::NULL => return None,
        Value::Bytes(b) => b.clone(),
        Value::Int(i) => i.to_string().into_bytes(),
        Value::UInt(u) => u.to_string().into_bytes(),
        Value::Float(f) => f.to_string().into_bytes(),
        Value::Double(d) => d.to_string().into_bytes(),
        Value::Date(y, mo, d, h, mi, s, us) => {
            let mut t = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}");
            if *us > 0 {
                t.push_str(&format!(".{us:06}"));
            }
            t.into_bytes()
        }
        Value::Time(neg, days, h, mi, s, us) => {
            let hours = u64::from(*days) * 24 + u64::from(*h);
            let mut t = format!("{}{hours:02}:{mi:02}:{s:02}", if *neg { "-" } else { "" });
            if *us > 0 {
                t.push_str(&format!(".{us:06}"));
            }
            t.into_bytes()
        }
    })
}
