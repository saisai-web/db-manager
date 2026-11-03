//! ER 図の表示。図はキャッシュ済みのスキーマ情報から、描くたびに作る(DB には問い合わせない)。

use gpui_kit::Context;
use sqltool_core::{ErGraph, TabId, build_er_graph};

use super::{AppModel, SchemaState};

/// 図にするスキーマと、そのうち列の情報が届いた表の数。
pub struct ErProgress {
    pub loaded: usize,
    pub total: usize,
}

/// スキーマの表と外部キーを、図のデータにする。列の情報が届いていない表は含まれない。
pub fn graph_of(state: &SchemaState, schema: &str) -> (ErGraph, ErProgress) {
    let tables = state.tables.get(schema).map_or(&[][..], Vec::as_slice);
    let details: Vec<_> = tables
        .iter()
        .filter_map(|t| state.details.get(&(schema.to_string(), t.name.clone())))
        .collect();
    let progress = ErProgress {
        loaded: details.len(),
        total: tables.len(),
    };
    (build_er_graph(details), progress)
}

impl AppModel {
    pub fn show_er(&mut self, tab: TabId, schema: String, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(tab) {
            t.er = Some(schema);
        }
        cx.notify();
    }

    pub fn close_er(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(tab) {
            t.er = None;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqltool_core::{ColumnInfo, TableDetails, TableInfo, TableKind, TableRef};

    #[test]
    fn tables_without_details_are_counted_but_not_drawn() {
        let mut s = SchemaState::default();
        let info = |n: &str| TableInfo {
            schema: "public".into(),
            name: n.into(),
            kind: TableKind::Table,
        };
        s.tables.insert("public".into(), vec![info("a"), info("b")]);
        s.details.insert(
            ("public".into(), "a".into()),
            TableDetails {
                table: TableRef::new("public", "a"),
                columns: vec![ColumnInfo {
                    name: "id".into(),
                    type_name: "int".into(),
                    nullable: false,
                    default: None,
                    ordinal: 1,
                }],
                keys: vec![],
                indexes: vec![],
            },
        );
        let (g, p) = graph_of(&s, "public");
        assert_eq!((g.tables.len(), p.loaded, p.total), (1, 1, 2));
        let (g, p) = graph_of(&s, "nope");
        assert_eq!((g.tables.len(), p.loaded, p.total), (0, 0, 0));
    }
}
