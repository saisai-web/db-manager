//! 論理型と既定値を、DB ごとの書き方にする。

use super::Target;
use crate::types::DataType;

/// DB ごとの型。変換で意味が変わるときは、警告の文を返す。
pub fn native_type(ty: &DataType, target: Target) -> (String, Option<String>) {
    use DataType as T;
    use Target::*;
    let sized = |name: &str, n: &Option<u32>, default: Option<u32>| match n.or(default) {
        Some(n) => format!("{name}({n})"),
        None => name.to_string(),
    };
    let numeric = |name: &str, p: &Option<u32>, s: &Option<u32>| match (p, s) {
        (Some(p), Some(s)) => format!("{name}({p},{s})"),
        (Some(p), None) => format!("{name}({p})"),
        _ => name.to_string(),
    };
    let t = match (ty, target) {
        (T::Native(s), _) => s.clone(),

        (T::SmallInt, Postgres | MySql | SqlServer) => "SMALLINT".into(),
        (T::SmallInt, Oracle) => "NUMBER(5)".into(),
        (T::Int, Postgres) => "INTEGER".into(),
        (T::Int, MySql | SqlServer) => "INT".into(),
        (T::Int, Oracle) => "NUMBER(10)".into(),
        (T::BigInt, Postgres | MySql | SqlServer) => "BIGINT".into(),
        (T::BigInt, Oracle) => "NUMBER(19)".into(),
        (T::SmallInt | T::Int | T::BigInt, Sqlite) => "INTEGER".into(),

        (T::Decimal { precision, scale }, Postgres) => numeric("NUMERIC", precision, scale),
        (T::Decimal { precision, scale }, MySql | SqlServer) => {
            numeric("DECIMAL", precision, scale)
        }
        (T::Decimal { precision, scale }, Oracle) => numeric("NUMBER", precision, scale),
        (T::Decimal { precision, scale }, Sqlite) => numeric("NUMERIC", precision, scale),

        (T::Float, Postgres | SqlServer | Sqlite) => "REAL".into(),
        (T::Float, MySql) => "FLOAT".into(),
        (T::Float, Oracle) => "BINARY_FLOAT".into(),
        (T::Double, Postgres) => "DOUBLE PRECISION".into(),
        (T::Double, MySql) => "DOUBLE".into(),
        (T::Double, SqlServer) => "FLOAT(53)".into(),
        (T::Double, Sqlite) => "REAL".into(),
        (T::Double, Oracle) => "BINARY_DOUBLE".into(),

        (T::Bool, Postgres | MySql) => "BOOLEAN".into(),
        (T::Bool, SqlServer) => "BIT".into(),
        (T::Bool, Sqlite) => "BOOLEAN".into(),
        (T::Bool, Oracle) => "NUMBER(1)".into(),

        (T::Char(n), Oracle) => match n {
            Some(n) => format!("CHAR({n} CHAR)"),
            None => "CHAR(1 CHAR)".into(),
        },
        (T::Char(n), SqlServer) => sized("NCHAR", n, Some(1)),
        (T::Char(n), _) => sized("CHAR", n, None),
        (T::Varchar(n), Oracle) => format!("VARCHAR2({} CHAR)", n.unwrap_or(255)),
        (T::Varchar(n), SqlServer) => format!("NVARCHAR({})", n.unwrap_or(255)),
        (T::Varchar(n), MySql) => format!("VARCHAR({})", n.unwrap_or(255)),
        (T::Varchar(n), Postgres | Sqlite) => sized("VARCHAR", n, None),
        (T::Text, Postgres | MySql | Sqlite) => "TEXT".into(),
        (T::Text, SqlServer) => "NVARCHAR(MAX)".into(),
        (T::Text, Oracle) => "CLOB".into(),

        (T::Date, _) => "DATE".into(),
        (T::Time, Oracle) => "VARCHAR2(8)".into(),
        (T::Time, _) => "TIME".into(),
        (T::DateTime, Postgres | Oracle) => "TIMESTAMP".into(),
        (T::DateTime, MySql | Sqlite) => "DATETIME".into(),
        (T::DateTime, SqlServer) => "DATETIME2".into(),
        (T::DateTimeTz, Postgres) => "TIMESTAMPTZ".into(),
        (T::DateTimeTz, MySql) => "TIMESTAMP".into(),
        (T::DateTimeTz, SqlServer) => "DATETIMEOFFSET".into(),
        (T::DateTimeTz, Sqlite) => "DATETIME".into(),
        (T::DateTimeTz, Oracle) => "TIMESTAMP WITH TIME ZONE".into(),

        (T::Uuid, Postgres) => "UUID".into(),
        (T::Uuid, MySql | Sqlite) => "CHAR(36)".into(),
        (T::Uuid, SqlServer) => "UNIQUEIDENTIFIER".into(),
        (T::Uuid, Oracle) => "RAW(16)".into(),
        (T::Json, Postgres) => "JSONB".into(),
        (T::Json, MySql) => "JSON".into(),
        (T::Json, SqlServer) => "NVARCHAR(MAX)".into(),
        (T::Json, Sqlite) => "TEXT".into(),
        (T::Json, Oracle) => "CLOB".into(),
        (T::Blob, Postgres) => "BYTEA".into(),
        (T::Blob, MySql | Sqlite | Oracle) => "BLOB".into(),
        (T::Blob, SqlServer) => "VARBINARY(MAX)".into(),
    };
    (t, warning(ty, target))
}

fn warning(ty: &DataType, target: Target) -> Option<String> {
    match (ty, target) {
        (DataType::Time, Target::Oracle) => {
            Some("Oracle には TIME 型がないため、VARCHAR2(8) にしました".into())
        }
        (DataType::Json, Target::SqlServer | Target::Oracle | Target::Sqlite) => {
            Some("JSON 型を持たない DB のため、文字列の型にしました".into())
        }
        (DataType::Uuid, Target::MySql | Target::Sqlite | Target::Oracle) => {
            Some("UUID 型を持たない DB のため、固定長の型にしました".into())
        }
        (DataType::DateTimeTz, Target::MySql | Target::Sqlite) => {
            Some("タイムゾーンつきの日時型を持たない DB のため、通常の日時型にしました".into())
        }
        (DataType::Varchar(Some(n)), Target::Oracle) if *n > 4000 => Some(format!(
            "Oracle の VARCHAR2 は 4000 まで(指定 {n})。CLOB にしてください"
        )),
        _ => None,
    }
}

/// 既定値の式を、その DB で通る書き方にする。引用符つきの文字列と数値はそのまま。
pub fn default_expr(value: &str, target: Target) -> String {
    use Target::*;
    let v = value.trim();
    match v.to_ascii_lowercase().as_str() {
        "now()" | "current_timestamp" | "current_timestamp()" | "getdate()" | "sysdate" => {
            "CURRENT_TIMESTAMP".into()
        }
        "current_date" | "current_date()" => match target {
            SqlServer => "CAST(GETDATE() AS DATE)".into(),
            _ => "CURRENT_DATE".into(),
        },
        "true" => match target {
            Postgres | MySql => "TRUE".into(),
            _ => "1".into(),
        },
        "false" => match target {
            Postgres | MySql => "FALSE".into(),
            _ => "0".into(),
        },
        "uuid()" | "gen_random_uuid()" | "newid()" => match target {
            Postgres => "gen_random_uuid()".into(),
            MySql => "(UUID())".into(),
            SqlServer => "NEWID()".into(),
            Oracle => "SYS_GUID()".into(),
            Sqlite => "(lower(hex(randomblob(16))))".into(),
        },
        _ if target == Sqlite && v.contains('(') && !v.starts_with('(') => format!("({v})"),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DataType;

    fn ty(s: &str, target: Target) -> String {
        native_type(&DataType::parse(s), target).0
    }

    #[test]
    fn the_same_logical_type_maps_to_each_databases_own_type() {
        let row = |s| Target::ALL.map(|t| ty(s, t)).join(" | ");
        assert_eq!(
            row("int"),
            "INTEGER | INT | INTEGER | INT | NUMBER(10)",
            "order: postgres, mysql, sqlite, sqlserver, oracle"
        );
        assert_eq!(
            row("varchar(100)"),
            "VARCHAR(100) | VARCHAR(100) | VARCHAR(100) | NVARCHAR(100) | VARCHAR2(100 CHAR)"
        );
        assert_eq!(
            row("decimal(10,2)"),
            "NUMERIC(10,2) | DECIMAL(10,2) | NUMERIC(10,2) | DECIMAL(10,2) | NUMBER(10,2)"
        );
        assert_eq!(
            row("datetime"),
            "TIMESTAMP | DATETIME | DATETIME | DATETIME2 | TIMESTAMP"
        );
        assert_eq!(row("bool"), "BOOLEAN | BOOLEAN | BOOLEAN | BIT | NUMBER(1)");
        assert_eq!(row("text"), "TEXT | TEXT | TEXT | NVARCHAR(MAX) | CLOB");
    }

    #[test]
    fn varchar_without_a_length_gets_a_length_where_the_database_needs_one() {
        assert_eq!(ty("varchar", Target::Postgres), "VARCHAR");
        assert_eq!(ty("varchar", Target::MySql), "VARCHAR(255)");
        assert_eq!(ty("varchar", Target::SqlServer), "NVARCHAR(255)");
    }

    #[test]
    fn native_types_pass_through_and_lossy_mappings_warn() {
        assert_eq!(ty("citext", Target::Postgres), "citext");
        assert!(native_type(&DataType::Time, Target::Oracle).1.is_some());
        assert!(native_type(&DataType::Json, Target::Sqlite).1.is_some());
        assert!(native_type(&DataType::Int, Target::Oracle).1.is_none());
        assert!(
            native_type(&DataType::Varchar(Some(5000)), Target::Oracle)
                .1
                .is_some()
        );
    }

    #[test]
    fn defaults_are_translated_per_database() {
        assert_eq!(default_expr("now()", Target::Oracle), "CURRENT_TIMESTAMP");
        assert_eq!(default_expr("true", Target::SqlServer), "1");
        assert_eq!(default_expr("true", Target::Postgres), "TRUE");
        assert_eq!(default_expr("'A'", Target::Postgres), "'A'");
        assert_eq!(default_expr("0", Target::MySql), "0");
        assert_eq!(default_expr("uuid()", Target::SqlServer), "NEWID()");
        assert_eq!(default_expr("lower('X')", Target::Sqlite), "(lower('X'))");
        assert_eq!(default_expr("lower('X')", Target::Postgres), "lower('X')");
    }
}
