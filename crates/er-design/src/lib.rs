//! ER 図の設計。表・列・関係のモデルと、それを素早く書くためのテキスト記法、
//! DB ごとの CREATE 文、図の配置と描画(SVG)、設計書(Excel / HTML)の書き出し。
//! どれも DB と画面に依存しない純粋なロジック。

pub mod columns;
pub mod ddl;
pub mod diagram;
pub mod docs;
pub mod dsl;
pub mod edit;
mod model;
pub mod relations;
pub mod svg;
mod types;

pub use model::{
    Column, Design, ForeignKey, Index, MAX_SCALE, MIN_SCALE, RefAction, Table, UniqueKey,
};
pub use types::DataType;
mod validate;

pub use validate::{Issue, Severity, validate};
