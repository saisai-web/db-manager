//! DB ごとの CREATE 文。設計(論理型)から、対象の DB の型・引用符・制約の書き方で作る。
//! 表は外部キーの依存順に並べ、循環する参照は、全表を作ったあとの ALTER TABLE にする。

mod cascade;
mod extras;
mod names;
mod native;
mod order;
mod table;
#[cfg(test)]
mod tests;

use crate::model::Design;

pub use native::default_expr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Postgres,
    MySql,
    Sqlite,
    SqlServer,
    Oracle,
}

impl Target {
    pub const ALL: [Target; 5] = [
        Target::Postgres,
        Target::MySql,
        Target::Sqlite,
        Target::SqlServer,
        Target::Oracle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::MySql => "MySQL / MariaDB",
            Self::Sqlite => "SQLite",
            Self::SqlServer => "SQL Server",
            Self::Oracle => "Oracle",
        }
    }

    /// 接続の種類(`ConnectionSummary::driver`)から、対応する出力先を選ぶ。
    pub fn from_driver(driver: &str) -> Option<Target> {
        Some(match driver {
            "postgresql" | "postgres" => Self::Postgres,
            "mysql" | "mariadb" => Self::MySql,
            "sqlite" => Self::Sqlite,
            "sqlserver" | "mssql" => Self::SqlServer,
            "oracle" => Self::Oracle,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdlOptions {
    /// 表を作るスキーマ(空なら指定しない)
    pub schema: String,
    /// 先に既存の表を DROP する
    pub drop_first: bool,
    /// 論理名・備考を、コメントとして DB に残す
    pub comments: bool,
    /// 外部キーの列に索引を付ける(MySQL は自動で付くので不要)
    pub index_foreign_keys: bool,
}

impl Default for DdlOptions {
    fn default() -> Self {
        Self {
            schema: String::new(),
            drop_first: false,
            comments: true,
            index_foreign_keys: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ddl {
    pub sql: String,
    /// DB の制約で、設計どおりには作れなかった点
    pub warnings: Vec<String>,
}

/// 表を作るスキーマ。SQLite にはスキーマがないので、空。
pub(crate) fn schema_for(target: Target, opts: &DdlOptions) -> &str {
    if target == Target::Sqlite {
        ""
    } else {
        &opts.schema
    }
}

pub fn generate(design: &Design, target: Target, opts: &DdlOptions) -> Ddl {
    let mut out = Ddl::default();
    if target == Target::Sqlite && !opts.schema.is_empty() {
        out.warnings
            .push("SQLite にはスキーマがないため、スキーマ名は付けません".into());
    }
    let order = order::creation_order(design);
    let mut parts: Vec<String> = Vec::new();

    parts.push(format!(
        "-- {}({})",
        if design.title.is_empty() {
            "ER 設計"
        } else {
            &design.title
        },
        target.label()
    ));
    if let Some(s) = extras::create_schema(target, opts) {
        parts.push(s);
    }
    if opts.drop_first {
        parts.extend(extras::drops(design, &order, target, opts));
    }

    let rejected = if target == Target::SqlServer {
        cascade::sqlserver_rejected(design, &order)
    } else {
        Default::default()
    };
    let mut deferred = Vec::new();
    let mut created = std::collections::HashSet::new();
    for &i in &order.tables {
        let t = &design.tables[i];
        let (sql, later) = table::create_table(
            design,
            t,
            target,
            opts,
            &created,
            &rejected,
            &mut out.warnings,
        );
        created.insert(t.name.as_str());
        parts.push(sql);
        deferred.extend(later);
        parts.extend(extras::indexes(design, t, target, opts));
    }
    parts.extend(deferred);
    if opts.comments {
        parts.extend(extras::comments(design, target, opts));
    }
    extras::check_name_lengths(design, target, &mut out.warnings);

    out.sql = parts.join("\n\n");
    out.sql.push('\n');
    out
}
