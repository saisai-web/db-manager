//! ER 設計の画面を開く。接続中のスキーマの表を、設計に取り込む。

use std::path::PathBuf;

use gpui_kit::{AppContext as _, Context, Window};
use sqltool_core::design::ddl::Target;
use sqltool_core::{Command, TabId, TableKind, TableRef, design_from_details};

use super::{AppModel, StatusLine};
use crate::ui::Designer;

impl AppModel {
    /// 出力先の DB の初期値: 開いている接続の種類(なければ PostgreSQL)。
    fn default_design_target(&self) -> Target {
        self.active_tab()
            .and_then(|t| Target::from_driver(&t.connection.driver))
            .unwrap_or(Target::Postgres)
    }

    /// ER 設計を開く。初めてなら作る(前回の下書きがあれば、その続きから)。
    pub fn open_designer(
        &mut self,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let designer = match &self.designer {
            Some(d) => d.clone(),
            None => {
                let model = cx.entity();
                let target = self.default_design_target();
                let d = cx.new(|cx| Designer::new(model, target, window, cx));
                if file.is_none() {
                    d.update(cx, |d, cx| d.restore_draft(window, cx));
                }
                self.designer = Some(d.clone());
                d
            }
        };
        designer.update(cx, |d, cx| {
            d.visible = true;
            if let Some(f) = file {
                d.open_path(f, window, cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    /// スキーマの表と関係を、ER 設計に取り込む。列の情報がまだ届いていない表があれば、
    /// 読み込みを頼んで、少し待ってからもう一度押してもらう。
    pub fn import_schema_to_designer(
        &mut self,
        tab: TabId,
        schema: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.tab(tab) else { return };
        let (connection, name) = (t.connection.id.clone(), t.connection.name.clone());
        let Some(state) = self.schemas.get(&connection) else {
            return;
        };
        let tables: Vec<_> = state
            .tables
            .get(&schema)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(|t| t.kind == TableKind::Table)
            .collect();
        let details: Vec<_> = tables
            .iter()
            .filter_map(|t| state.details.get(&(schema.clone(), t.name.clone())))
            .cloned()
            .collect();
        if details.len() < tables.len() {
            for t in tables.iter().filter(|t| {
                !state
                    .details
                    .contains_key(&(schema.clone(), t.name.clone()))
            }) {
                self.core.send(Command::LoadTableDetails {
                    connection_id: connection.clone(),
                    table: TableRef::new(&schema, &t.name),
                });
            }
            self.notice = Some(StatusLine::ok(format!(
                "表の情報を読み込み中です({}/{})。少し待って、もう一度「設計」を押してください",
                details.len(),
                tables.len()
            )));
            cx.notify();
            return;
        }
        let design = design_from_details(&format!("{name} / {schema}"), &details);
        self.open_designer(None, window, cx);
        if let Some(d) = self.designer.clone() {
            d.update(cx, |d, cx| {
                d.import(design, &format!("{name} の {schema}"), window, cx)
            });
        }
    }
}
