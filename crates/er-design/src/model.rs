//! 設計のデータ。表・列・外部キー・索引。DB に依存しない(DB ごとの違いは DDL にするときに吸収する)。

use crate::types::DataType;

/// 表の箱の大きさの範囲。文字の大きさは変えず、余白が増える(1.0 は、中身にぴったり。それより小さくはしない)。
pub const MIN_SCALE: f32 = 1.0;
pub const MAX_SCALE: f32 = 2.5;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Design {
    pub title: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// 物理名
    pub name: String,
    /// 論理名(日本語名など)
    pub logical: String,
    /// 備考・用途
    pub comment: String,
    pub columns: Vec<Column>,
    pub uniques: Vec<UniqueKey>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
    /// 図の中の位置(左上)。`None` なら自動配置
    pub pos: Option<(f32, f32)>,
    /// 箱の拡大率。`None` なら等倍
    pub scale: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub logical: String,
    pub comment: String,
    pub ty: DataType,
    pub nullable: bool,
    pub primary: bool,
    pub unique: bool,
    /// 自動採番
    pub auto: bool,
    /// 既定値(SQL の式。文字列は `'...'`)
    pub default: Option<String>,
}

impl Column {
    pub fn new(name: impl Into<String>, ty: DataType) -> Self {
        Self {
            name: name.into(),
            logical: String::new(),
            comment: String::new(),
            ty,
            nullable: true,
            primary: false,
            unique: false,
            auto: false,
            default: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UniqueKey {
    /// 空なら自動で付ける
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RefAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl RefAction {
    pub fn sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }

    /// 設計書に書く日本語。
    pub fn label(self) -> &'static str {
        match self {
            Self::NoAction => "なし",
            Self::Restrict => "制限(RESTRICT)",
            Self::Cascade => "連鎖(CASCADE)",
            Self::SetNull => "NULL にする(SET NULL)",
            Self::SetDefault => "既定値にする(SET DEFAULT)",
        }
    }
}

/// 外部キー。この表(子)の `columns` が、`ref_table` の `ref_columns`(親)を参照する。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignKey {
    /// 空なら自動で付ける
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    /// 空なら、参照先の主キー
    pub ref_columns: Vec<String>,
    pub on_delete: RefAction,
    pub on_update: RefAction,
}

impl Design {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }

    pub fn table_index(&self, name: &str) -> Option<usize> {
        self.tables.iter().position(|t| t.name == name)
    }

    /// 外部キーの参照先の列。省略されていれば、参照先の主キー。
    pub fn ref_columns(&self, fk: &ForeignKey) -> Vec<String> {
        if !fk.ref_columns.is_empty() {
            return fk.ref_columns.clone();
        }
        self.table(&fk.ref_table)
            .map(|t| t.primary_key())
            .unwrap_or_default()
    }

    /// 表を削除する。その表を参照する外部キーも消す。
    pub fn remove_table(&mut self, name: &str) {
        self.tables.retain(|t| t.name != name);
        for t in &mut self.tables {
            t.foreign_keys.retain(|f| f.ref_table != name);
        }
    }

    /// 表の名前を変える。参照している外部キーも追従する。
    pub fn rename_table(&mut self, from: &str, to: &str) {
        for t in &mut self.tables {
            if t.name == from {
                t.name = to.to_string();
            }
            for f in &mut t.foreign_keys {
                if f.ref_table == from {
                    f.ref_table = to.to_string();
                }
            }
        }
    }

    /// 使われていない名前を返す(`base`、`base_2`、…)。
    pub fn unused_table_name(&self, base: &str) -> String {
        if self.table(base).is_none() {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}_{n}"))
            .find(|n| self.table(n).is_none())
            .unwrap_or_default()
    }
}

impl Table {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// 主キーの列名(列の並び順)。
    pub fn primary_key(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|c| c.primary)
            .map(|c| c.name.clone())
            .collect()
    }

    /// 箱の拡大率(範囲に収める)。
    pub fn factor(&self) -> f32 {
        self.scale.map_or(1.0, |s| s.clamp(MIN_SCALE, MAX_SCALE))
    }

    /// 論理名があれば論理名、なければ物理名。
    pub fn display_name(&self) -> &str {
        if self.logical.is_empty() {
            &self.name
        } else {
            &self.logical
        }
    }

    /// 列が外部キーに使われているか。
    pub fn is_foreign(&self, column: &str) -> bool {
        self.foreign_keys
            .iter()
            .any(|f| f.columns.iter().any(|c| c == column))
    }

    /// 外部キーの名前。指定がなければ `fk_表_列…`。
    pub fn fk_name(&self, fk: &ForeignKey) -> String {
        if fk.name.is_empty() {
            format!("fk_{}_{}", self.name, fk.columns.join("_"))
        } else {
            fk.name.clone()
        }
    }

    pub fn pk_name(&self) -> String {
        format!("pk_{}", self.name)
    }

    pub fn unique_name(&self, u: &UniqueKey) -> String {
        if u.name.is_empty() {
            format!("uq_{}_{}", self.name, u.columns.join("_"))
        } else {
            u.name.clone()
        }
    }

    pub fn index_name(&self, i: &Index) -> String {
        if i.name.is_empty() {
            let prefix = if i.unique { "ux" } else { "ix" };
            format!("{prefix}_{}_{}", self.name, i.columns.join("_"))
        } else {
            i.name.clone()
        }
    }

    /// 外部キーの列がすべて NOT NULL か(親が必須の関係か)。
    pub fn fk_required(&self, fk: &ForeignKey) -> bool {
        fk.columns
            .iter()
            .all(|c| self.column(c).is_some_and(|c| !c.nullable || c.primary))
    }

    /// 外部キーの列が、そのまま主キーか一意になっているか(1 対 1 の関係か)。
    pub fn fk_is_one_to_one(&self, fk: &ForeignKey) -> bool {
        let pk = self.primary_key();
        let same = |cols: &[String]| {
            cols.len() == fk.columns.len() && fk.columns.iter().all(|c| cols.contains(c))
        };
        same(&pk)
            || self.uniques.iter().any(|u| same(&u.columns))
            || (fk.columns.len() == 1 && self.column(&fk.columns[0]).is_some_and(|c| c.unique))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk_col(name: &str) -> Column {
        Column {
            primary: true,
            nullable: false,
            ..Column::new(name, DataType::Int)
        }
    }

    #[test]
    fn omitted_reference_columns_mean_the_primary_key_of_the_parent() {
        let mut parent = Table::new("orgs");
        parent.columns = vec![pk_col("id"), Column::new("name", DataType::Text)];
        let d = Design {
            tables: vec![parent],
            ..Default::default()
        };
        let fk = ForeignKey {
            columns: vec!["org_id".into()],
            ref_table: "orgs".into(),
            ..Default::default()
        };
        assert_eq!(d.ref_columns(&fk), ["id"]);
        let missing = ForeignKey {
            ref_table: "nope".into(),
            ..fk
        };
        assert!(d.ref_columns(&missing).is_empty());
    }

    #[test]
    fn removing_or_renaming_a_table_updates_the_references_to_it() {
        let mut child = Table::new("users");
        child.foreign_keys = vec![ForeignKey {
            columns: vec!["org_id".into()],
            ref_table: "orgs".into(),
            ..Default::default()
        }];
        let mut d = Design {
            tables: vec![Table::new("orgs"), child],
            ..Default::default()
        };
        d.rename_table("orgs", "companies");
        assert_eq!(d.tables[1].foreign_keys[0].ref_table, "companies");
        d.remove_table("companies");
        assert_eq!(d.tables.len(), 1);
        assert!(d.tables[0].foreign_keys.is_empty());
    }

    #[test]
    fn unused_names_count_up() {
        let d = Design {
            tables: vec![Table::new("t"), Table::new("t_2")],
            ..Default::default()
        };
        assert_eq!(d.unused_table_name("x"), "x");
        assert_eq!(d.unused_table_name("t"), "t_3");
    }

    #[test]
    fn relation_kinds_come_from_nullability_and_keys() {
        let mut t = Table::new("profiles");
        t.columns = vec![pk_col("user_id"), Column::new("org_id", DataType::Int)];
        let one = ForeignKey {
            columns: vec!["user_id".into()],
            ref_table: "users".into(),
            ..Default::default()
        };
        let many = ForeignKey {
            columns: vec!["org_id".into()],
            ref_table: "orgs".into(),
            ..Default::default()
        };
        assert!(t.fk_is_one_to_one(&one) && !t.fk_is_one_to_one(&many));
        assert!(t.fk_required(&one) && !t.fk_required(&many));
        assert_eq!(t.fk_name(&many), "fk_profiles_org_id");
    }
}
