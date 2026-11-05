//! ER 設計の画面。画面いっぱいに図を出し、表はその場で編集できる(列名・型をクリック、見出しの「＋」で列を足す、
//! ダブルクリックか「詳細」で全項目の編集、右下をドラッグで表の大きさ)。CREATE 文・点検・設計書の書き出しは、
//! 「SQL・書き出し」のダイアログに出す。テキスト記法は、必要なときだけ「テキスト」で左に開く。
//! 図とテキストは同じ設計の 2 つの見え方で、どちらを編集しても、もう一方に反映される。
//!
//! 状態は `AppModel::designer` が持つ。画面を閉じても設計は残り、「ER設計」で続きから開ける。

mod bar;
mod detail;
mod detail_view;
mod diagram;
mod dialog;
mod drag;
mod files;
mod inline;
mod relation;
mod relation_view;
mod side;
mod state;
mod table_box;
mod type_menu;
mod view;

use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::component::input::{EditorState, InputState};
use sqltool_core::design::ddl::{Ddl, DdlOptions, Target};
use sqltool_core::design::diagram::{Geometry, Labels};
use sqltool_core::design::edit::Pick;
use sqltool_core::design::{Design, Issue};

use self::detail::Detail;
use self::inline::Inline;
use self::relation::Relation;
use self::type_menu::TypeMenu;
use super::prelude::*;
use crate::model::AppModel;

/// 右側のパネル。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SidePanel {
    Sql,
    Check,
    Docs,
}

/// 設計から導いた、描画のための値。設計が変わるたびに作り直す。
pub(super) struct Derived {
    pub geo: Geometry,
    /// CREATE 文は、書き出しのダイアログを開いているときだけ作る(設計を触るたびには作らない)
    pub ddl: Ddl,
    /// CREATE 文の全文(コピー・保存・SQL エディタへ渡す)
    pub sql: SharedString,
    /// 画面に出す分(長いときは先頭だけ)
    pub preview: SharedString,
    pub issues: Vec<Issue>,
}

/// 表の移動中の状態。
pub(super) struct Drag {
    pub table: usize,
    pub start_mouse: (f32, f32),
    pub start_pos: (f32, f32),
    /// 動かす前の設計のテキスト(元に戻すため)
    pub before: String,
    pub moved: bool,
}

/// 表の大きさを変えている途中の状態。
pub(super) struct Resize {
    pub table: usize,
    pub start_mouse_x: f32,
    pub start_scale: f32,
    /// 動かし始めた時点の、画面上の箱の幅(px)
    pub start_width: f32,
    pub before: String,
    pub moved: bool,
}

pub struct Designer {
    pub(super) model: Entity<AppModel>,
    pub visible: bool,
    pub(super) design: Design,
    pub(super) text: Entity<EditorState>,
    pub(super) schema_input: Entity<InputState>,
    /// 設計に対応するテキスト。エディタの内容がこれと違えば、利用者が入力した
    pub(super) last_text: String,
    /// テキストの誤り(直るまで、図は直前の正しい設計のまま)
    pub(super) error: Option<String>,
    pub(super) derived: Derived,
    pub(super) selected: Option<String>,
    pub(super) connect: bool,
    pub(super) pick: Option<Pick>,
    pub(super) drag: Option<Drag>,
    pub(super) resize: Option<Resize>,
    /// 図の上でその場編集している欄(列名・型・表名)
    pub(super) inline: Option<Inline>,
    /// 表の詳細エディタ(開いているとき)
    pub(super) detail: Option<Detail>,
    /// 関係線の詳細ダイアログ(開いているとき)
    pub(super) relation: Option<Relation>,
    /// 開いている型の一覧
    pub(super) type_menu: Option<TypeMenu>,
    /// 左にテキスト記法を出すか
    pub(super) show_text: bool,
    /// 「SQL・書き出し」のダイアログを開いているか
    pub(super) export_open: bool,
    pub(super) zoom: f32,
    pub(super) labels: Labels,
    pub(super) target: Target,
    pub(super) options: DdlOptions,
    pub(super) panel: SidePanel,
    pub(super) appendix_ddl: bool,
    pub(super) undo: Vec<String>,
    pub(super) redo: Vec<String>,
    pub(super) last_record: Option<Instant>,
    pub(super) file: Option<PathBuf>,
    pub(super) dirty: bool,
    /// 画面下に出す通知(誤りなら `true`)
    pub(super) message: Option<(bool, String)>,
    pub(super) draft_seq: u64,
    /// 下書きを自動で保存する(テストでは保存しない)
    pub(super) autosave: bool,
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_edit;
#[cfg(test)]
mod tests_relation;
