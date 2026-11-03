//! 変更から、実行する SQL とバインド値を作る。DB の違いは `Dialect` に任せる。

use driver_api::{ColumnInfo, Dialect};

use super::change::{Change, ChangeKind, Value};
use super::source::TableSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEdit {
    pub kind: ChangeKind,
    /// 実行する SQL(値はバインドで渡す)
    pub sql: String,
    pub params: Vec<Value>,
    /// 利用者に見せる SQL(値を埋め込んだもの。実行には使わない)
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("保存する変更がありません")]
    NoChanges,
    #[error("列「{0}」はこのテーブルにありません")]
    UnknownColumn(String),
    #[error("キーの値が NULL の行は、特定できないため更新・削除できません")]
    NullKey,
    #[error("キー列が指定されていません")]
    EmptyKey,
}

/// 変更を、1 行 1 文の SQL にする。
pub fn plan(
    source: &TableSource,
    changes: &[Change],
    dialect: &dyn Dialect,
) -> Result<Vec<PlannedEdit>, EditError> {
    if changes.is_empty() {
        return Err(EditError::NoChanges);
    }
    changes
        .iter()
        .map(|c| plan_one(source, c, dialect))
        .collect()
}

/// 値の埋め込み方。バインド用(`$n`)と、プレビュー用(リテラル)で共通の SQL を組み立てる。
trait Render {
    fn value(&mut self, col: &ColumnInfo, v: &Value) -> String;
}

struct Binds<'a> {
    dialect: &'a dyn Dialect,
    params: Vec<Value>,
}

impl Render for Binds<'_> {
    fn value(&mut self, col: &ColumnInfo, v: &Value) -> String {
        self.params.push(v.clone());
        self.dialect.bind_text(self.params.len(), &col.type_name)
    }
}

struct Literals<'a> {
    dialect: &'a dyn Dialect,
}

impl Render for Literals<'_> {
    fn value(&mut self, col: &ColumnInfo, v: &Value) -> String {
        match v {
            None => "NULL".into(),
            Some(s) if is_plain_number(&col.type_name, s) => s.clone(),
            Some(s) => self.dialect.quote_literal(s),
        }
    }
}

/// 数値型の列で、そのまま書ける数値なら、引用符なしで見せる(プレビューを読みやすくするため)。
fn is_plain_number(type_name: &str, v: &str) -> bool {
    const NUMERIC: [&str; 7] = [
        "smallint", "integer", "bigint", "numeric", "decimal", "real", "double",
    ];
    NUMERIC.iter().any(|n| type_name.starts_with(n))
        && !v.is_empty()
        && v.parse::<f64>().is_ok_and(f64::is_finite)
        && v.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'.'))
}

fn plan_one(
    source: &TableSource,
    change: &Change,
    dialect: &dyn Dialect,
) -> Result<PlannedEdit, EditError> {
    let mut binds = Binds {
        dialect,
        params: Vec::new(),
    };
    let sql = build(source, change, dialect, &mut binds)?;
    let preview = build(source, change, dialect, &mut Literals { dialect })?;
    Ok(PlannedEdit {
        kind: change.kind(),
        sql,
        params: binds.params,
        preview,
    })
}

fn build(
    source: &TableSource,
    change: &Change,
    dialect: &dyn Dialect,
    r: &mut dyn Render,
) -> Result<String, EditError> {
    let table = dialect.qualified_name(&source.table);
    let column = |name: &str| {
        source
            .column(name)
            .ok_or_else(|| EditError::UnknownColumn(name.to_string()))
    };
    let key_clause =
        |key: &[(String, Value)], r: &mut dyn Render| -> Result<Vec<String>, EditError> {
            if key.is_empty() {
                return Err(EditError::EmptyKey);
            }
            key.iter()
                .map(|(name, v)| {
                    if v.is_none() {
                        return Err(EditError::NullKey);
                    }
                    let col = column(name)?;
                    Ok(format!(
                        "{} = {}",
                        dialect.quote_ident(name),
                        r.value(col, v)
                    ))
                })
                .collect()
        };
    match change {
        Change::Update { key, set } => {
            if set.is_empty() {
                return Err(EditError::NoChanges);
            }
            let assignments: Vec<String> = set
                .iter()
                .map(|(name, new, _)| {
                    Ok(format!(
                        "{} = {}",
                        dialect.quote_ident(name),
                        r.value(column(name)?, new)
                    ))
                })
                .collect::<Result<_, EditError>>()?;
            let mut conds = key_clause(key, r)?;
            // 変更した列の元の値も条件にする。他の人が先に更新した行を上書きしない
            for (name, _, old) in set {
                let lhs = dialect.quote_ident(name);
                let rhs = r.value(column(name)?, old);
                conds.push(dialect.null_safe_equals(&lhs, &rhs));
            }
            Ok(format!(
                "UPDATE {table} SET {} WHERE {}",
                assignments.join(", "),
                conds.join(" AND ")
            ))
        }
        Change::Delete { key } => {
            let conds = key_clause(key, r)?;
            Ok(format!("DELETE FROM {table} WHERE {}", conds.join(" AND ")))
        }
        Change::Insert { values } => {
            if values.is_empty() {
                return Ok(format!("INSERT INTO {table} DEFAULT VALUES"));
            }
            let names: Vec<String> = values.iter().map(|(n, _)| dialect.quote_ident(n)).collect();
            let exprs: Vec<String> = values
                .iter()
                .map(|(n, v)| Ok(r.value(column(n)?, v)))
                .collect::<Result<_, EditError>>()?;
            Ok(format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                names.join(", "),
                exprs.join(", ")
            ))
        }
    }
}
