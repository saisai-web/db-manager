//! 列の型。DB に依存しない「論理型」で持ち、DDL にするときに DB ごとの型へ写す。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    SmallInt,
    Int,
    BigInt,
    Decimal {
        precision: Option<u32>,
        scale: Option<u32>,
    },
    Float,
    Double,
    Bool,
    Char(Option<u32>),
    Varchar(Option<u32>),
    Text,
    Date,
    Time,
    DateTime,
    DateTimeTz,
    Uuid,
    Json,
    Blob,
    /// 論理型にない型(`citext` など)。そのまま DDL に出す。
    Native(String),
}

/// `varchar(100)` を (`varchar`, [100]) に分ける。引数がなければ空。
fn split_args(s: &str) -> Option<(String, Vec<u32>)> {
    let Some(open) = s.find('(') else {
        return Some((s.trim().to_ascii_lowercase(), Vec::new()));
    };
    let inner = s[open + 1..].strip_suffix(')')?;
    let args = inner
        .split(',')
        .map(|a| a.trim().parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    Some((s[..open].trim().to_ascii_lowercase(), args))
}

impl DataType {
    /// 型の書き方(別名を含む)から型を決める。未知の名前は [`DataType::Native`]。
    pub fn parse(s: &str) -> DataType {
        let s = s.trim();
        let Some((name, args)) = split_args(s) else {
            return DataType::Native(s.to_string());
        };
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let arg = |i: usize| args.get(i).copied();
        match (name.as_str(), args.len()) {
            ("smallint" | "int2" | "tinyint", 0) => DataType::SmallInt,
            ("int" | "integer" | "int4", 0) => DataType::Int,
            ("bigint" | "int8", 0) => DataType::BigInt,
            ("decimal" | "numeric" | "number", 0..=2) => DataType::Decimal {
                precision: arg(0),
                scale: arg(1),
            },
            ("float" | "real" | "float4", 0) => DataType::Float,
            ("double" | "double precision" | "float8", 0) => DataType::Double,
            ("bool" | "boolean" | "bit", 0) => DataType::Bool,
            ("char" | "character" | "bpchar" | "nchar", 0..=1) => DataType::Char(arg(0)),
            ("varchar" | "varchar2" | "nvarchar" | "string" | "character varying", 0..=1) => {
                DataType::Varchar(arg(0))
            }
            ("text" | "clob" | "longtext", 0) => DataType::Text,
            ("date", 0) => DataType::Date,
            ("time" | "time without time zone", 0) => DataType::Time,
            ("datetime" | "datetime2" | "timestamp" | "timestamp without time zone", 0) => {
                DataType::DateTime
            }
            ("timestamptz" | "datetimetz" | "datetimeoffset" | "timestamp with time zone", 0) => {
                DataType::DateTimeTz
            }
            ("uuid" | "guid" | "uniqueidentifier", 0) => DataType::Uuid,
            ("json" | "jsonb", 0) => DataType::Json,
            ("blob" | "bytea" | "binary" | "varbinary", 0) => DataType::Blob,
            _ => DataType::Native(s.to_string()),
        }
    }

    /// 文字列の型か(`DEFAULT` の書き方や、外部キーの型の比較に使う)。
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Char(_) | Self::Varchar(_) | Self::Text)
    }

    /// 整数の型か。
    pub fn is_integer(&self) -> bool {
        matches!(self, Self::SmallInt | Self::Int | Self::BigInt)
    }

    /// 外部キーで結ぶ列として、型が合っているか。整数どうしは、幅が違っても警告しない。
    pub fn compatible_with(&self, other: &DataType) -> bool {
        match (self, other) {
            (a, b) if a.is_integer() && b.is_integer() => true,
            (a, b) if a.is_text() && b.is_text() => true,
            (Self::Decimal { .. }, Self::Decimal { .. }) => true,
            (Self::Native(a), Self::Native(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sized = |f: &mut fmt::Formatter<'_>, name: &str, n: &Option<u32>| match n {
            Some(n) => write!(f, "{name}({n})"),
            None => f.write_str(name),
        };
        match self {
            Self::SmallInt => f.write_str("smallint"),
            Self::Int => f.write_str("int"),
            Self::BigInt => f.write_str("bigint"),
            Self::Decimal {
                precision: Some(p),
                scale: Some(s),
            } => write!(f, "decimal({p},{s})"),
            Self::Decimal {
                precision: Some(p), ..
            } => write!(f, "decimal({p})"),
            Self::Decimal { .. } => f.write_str("decimal"),
            Self::Float => f.write_str("float"),
            Self::Double => f.write_str("double"),
            Self::Bool => f.write_str("bool"),
            Self::Char(n) => sized(f, "char", n),
            Self::Varchar(n) => sized(f, "varchar", n),
            Self::Text => f.write_str("text"),
            Self::Date => f.write_str("date"),
            Self::Time => f.write_str("time"),
            Self::DateTime => f.write_str("datetime"),
            Self::DateTimeTz => f.write_str("timestamptz"),
            Self::Uuid => f.write_str("uuid"),
            Self::Json => f.write_str("json"),
            Self::Blob => f.write_str("blob"),
            Self::Native(s) => f.write_str(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_map_to_one_type_and_print_canonically() {
        for (src, want) in [
            ("INTEGER", "int"),
            ("int4", "int"),
            ("Varchar(100)", "varchar(100)"),
            ("string", "varchar"),
            ("numeric(10, 2)", "decimal(10,2)"),
            ("number(8)", "decimal(8)"),
            ("double precision", "double"),
            ("timestamp", "datetime"),
            ("TIMESTAMPTZ", "timestamptz"),
            ("jsonb", "json"),
            ("bytea", "blob"),
        ] {
            assert_eq!(DataType::parse(src).to_string(), want, "{src}");
        }
    }

    #[test]
    fn unknown_names_and_bad_arguments_are_kept_as_native_types() {
        assert_eq!(DataType::parse("citext"), DataType::Native("citext".into()));
        assert_eq!(
            DataType::parse("varchar(x)"),
            DataType::Native("varchar(x)".into())
        );
        assert_eq!(
            DataType::parse("int(11)"),
            DataType::Native("int(11)".into()),
            "an argument the logical type does not take is not silently dropped"
        );
        assert_eq!(
            DataType::parse("geometry(Point,4326)").to_string(),
            "geometry(Point,4326)"
        );
    }

    #[test]
    fn parse_then_print_is_stable() {
        for src in ["char(3)", "decimal(10,2)", "text", "uuid", "citext"] {
            let t = DataType::parse(src);
            assert_eq!(DataType::parse(&t.to_string()), t);
        }
    }

    #[test]
    fn foreign_key_type_compatibility() {
        assert!(DataType::Int.compatible_with(&DataType::BigInt));
        assert!(DataType::Varchar(Some(10)).compatible_with(&DataType::Char(Some(3))));
        assert!(!DataType::Int.compatible_with(&DataType::Uuid));
        assert!(!DataType::Int.compatible_with(&DataType::Varchar(None)));
    }
}
