//! セッション管理、クエリ実行、安全確認、コマンドとイベント。
//!
//! UI は [`Core`] に [`Command`] を送り、[`EventStream`] から [`Event`] を受け取る。
//! 結果の行は、イベントで渡される [`ResultHandle`] から表示範囲だけを読み出す。

mod batch;
mod browse;
mod completion;
mod config;
mod connections;
mod core;
mod edit;
mod edit_ops;
mod er;
mod execution;
mod explain;
mod explain_mysql;
mod explain_ops;
mod explain_oracle;
mod explain_sqlserver;
mod export_ops;
mod history;
mod import_ops;
mod outcome;
mod plan;
mod result;
mod results;
mod rules;
mod schema;
mod sink;
mod ssh;
mod storage_actor;
mod tab;
mod tabs;
mod types;

pub use crate::completion::{Candidate, CandidateKind, Catalog, CatalogTable, Suggestion};
pub use crate::config::{CoreConfig, DriverRegistry};
pub use crate::core::{Core, EventStream};
pub use crate::edit::{
    Change, ChangeKind, EditAccess, EditBuffer, EditError, PlannedEdit, TableSource, Value,
    plan as plan_edit,
};
pub use crate::er::{
    BOX_WIDTH, ErColumn, ErGraph, ErRelation, ErTable, GAP_X, HEADER_HEIGHT, Layout,
    MAX_COLUMNS_SHOWN, ROW_HEIGHT, box_height, build as build_er_graph, column_anchor,
    layout as layout_er,
};
pub use crate::explain::{Plan, PlanNode, parse_postgres as parse_explain_json};
pub use crate::plan::{
    Assessment, ConfirmItem, ConfirmKind, ExecuteTarget, Planned, Refusal, TxnState, assess, plan,
    summarize,
};
pub use crate::result::ResultHandle;
pub use crate::ssh::{HOST_KEY_CHANGED, HOST_KEY_UNKNOWN};
pub use crate::types::*;
pub use result_store::{Access, RowView, Stats};
