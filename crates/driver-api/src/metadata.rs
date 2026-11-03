use async_trait::async_trait;

use crate::{Result, TableRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaInfo {
    pub name: String,
    /// DB が持つ管理用のスキーマ(`pg_catalog` など)。ツリーでは畳んで表示する
    pub system: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    /// 型の表示名(`character varying(20)` など)
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    /// 1 から始まる列の位置
    pub ordinal: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Primary,
    Unique,
    Foreign,
    Check,
    Exclusion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInfo {
    pub name: String,
    pub kind: KeyKind,
    pub columns: Vec<String>,
    /// 外部キーの参照先や CHECK の式など、DB が示す定義
    pub definition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub method: String,
    pub definition: String,
}

/// スキーマ情報の取得。実行中のクエリに待たされないよう、`Session` とは別の接続で動く。
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn schemas(&self) -> Result<Vec<SchemaInfo>>;
    async fn tables(&self, schema: &str) -> Result<Vec<TableInfo>>;
    async fn columns(&self, table: &TableRef) -> Result<Vec<ColumnInfo>>;
    async fn keys(&self, table: &TableRef) -> Result<Vec<KeyInfo>>;
    async fn indexes(&self, table: &TableRef) -> Result<Vec<IndexInfo>>;
    async fn close(self: Box<Self>) -> Result<()>;
}
