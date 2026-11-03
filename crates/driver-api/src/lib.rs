//! DB ごとの差を閉じ込める共通インターフェース。
//!
//! `core` と UI は、DB の名前で分岐せず、この crate の trait と [`Capabilities`] だけを使う。
//! 他の内部 crate には依存しない。

mod config;
mod dialect;
mod error;
mod metadata;
mod session;

pub use config::{ConnectionConfig, Secret, TlsMode};
pub use dialect::{Dialect, LexicalFeatures, PlanFormat, TableRef};
pub use error::{DriverError, ErrorKind, Result};
pub use metadata::{
    ColumnInfo, IndexInfo, KeyInfo, KeyKind, MetadataProvider, SchemaInfo, TableInfo, TableKind,
};
pub use session::{
    Canceller, Capabilities, Driver, ExecSummary, Flow, ResultColumn, RowSink, ServerInfo, Session,
    Statement, ValueClass,
};
