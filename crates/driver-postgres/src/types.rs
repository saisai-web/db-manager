use driver_api::ValueClass;
use tokio_postgres::types::{Kind, Type};

/// 型を、画面とソートのための大まかな分類にする。
pub(crate) fn classify(ty: &Type) -> ValueClass {
    match ty.kind() {
        Kind::Domain(base) => return classify(base),
        Kind::Array(_) | Kind::Composite(_) | Kind::Range(_) | Kind::Multirange(_) => {
            return ValueClass::Other;
        }
        Kind::Enum(_) => return ValueClass::Text,
        _ => {}
    }
    match *ty {
        Type::INT2
        | Type::INT4
        | Type::INT8
        | Type::NUMERIC
        | Type::FLOAT4
        | Type::FLOAT8
        | Type::OID => ValueClass::Number,
        Type::BOOL => ValueClass::Bool,
        Type::DATE
        | Type::TIME
        | Type::TIMETZ
        | Type::TIMESTAMP
        | Type::TIMESTAMPTZ
        | Type::INTERVAL => ValueClass::Temporal,
        Type::BYTEA => ValueClass::Binary,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR | Type::UUID => {
            ValueClass::Text
        }
        _ => ValueClass::Other,
    }
}

/// `bytea_output = hex` の `\x…` をバイト列に戻す。形式が違えば `None`。
pub(crate) fn decode_bytea_hex(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("\\x")?.as_bytes();
    if hex.len() % 2 != 0 {
        return None;
    }
    let nibble = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    hex.as_chunks::<2>()
        .0
        .iter()
        .map(|p| Some(nibble(p[0])? << 4 | nibble(p[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytea_hex_decoding() {
        assert_eq!(decode_bytea_hex("\\x"), Some(vec![]));
        assert_eq!(decode_bytea_hex("\\x00ff10Ab"), Some(vec![0, 255, 16, 171]));
        assert_eq!(decode_bytea_hex("\\x0"), None);
        assert_eq!(decode_bytea_hex("\\xzz"), None);
        assert_eq!(decode_bytea_hex("abc"), None);
        // escape 形式(bytea_output = escape)は復号しない
        assert_eq!(decode_bytea_hex("a\\000b"), None);
    }

    #[test]
    fn classification() {
        assert_eq!(classify(&Type::NUMERIC), ValueClass::Number);
        assert_eq!(classify(&Type::INT8), ValueClass::Number);
        assert_eq!(classify(&Type::TIMESTAMPTZ), ValueClass::Temporal);
        assert_eq!(classify(&Type::BYTEA), ValueClass::Binary);
        assert_eq!(classify(&Type::BOOL), ValueClass::Bool);
        assert_eq!(classify(&Type::TEXT), ValueClass::Text);
        assert_eq!(classify(&Type::INT4_ARRAY), ValueClass::Other);
        assert_eq!(classify(&Type::JSONB), ValueClass::Other);
        // 金額型は "$1,234.00" のように 10 進数として読めないため数値にしない
        assert_eq!(classify(&Type::MONEY), ValueClass::Other);
    }
}
