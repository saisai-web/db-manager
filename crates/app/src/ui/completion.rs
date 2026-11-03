//! エディタの SQL 補完。候補は core の補完エンジンが決め、スキーマ情報は、モデルのキャッシュから読む。

use anyhow::Result;
use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Task, WeakEntity, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};
use sqltool_core::{CandidateKind, ConnectionId};

use crate::model::AppModel;

pub struct SqlCompletion {
    pub model: WeakEntity<AppModel>,
    pub connection: ConnectionId,
    pub driver: String,
}

fn kind(k: CandidateKind) -> CompletionItemKind {
    match k {
        CandidateKind::Keyword => CompletionItemKind::KEYWORD,
        CandidateKind::Schema => CompletionItemKind::MODULE,
        CandidateKind::Table => CompletionItemKind::STRUCT,
        CandidateKind::Column => CompletionItemKind::FIELD,
    }
}

impl CompletionProvider for SqlCompletion {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let Some(model) = self.model.upgrade() else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let (core, catalog) =
            model.read_with(cx, |m, _| (m.core.clone(), m.catalog_for(&self.connection)));
        let sql = text.to_string();
        let suggestion = core.suggest_completion(&self.driver, &sql, offset, &catalog);
        let range = Range {
            start: text.offset_to_position(suggestion.replace.start),
            end: text.offset_to_position(suggestion.replace.end),
        };
        let items = suggestion
            .candidates
            .into_iter()
            .map(|c| CompletionItem {
                kind: Some(kind(c.kind)),
                detail: c.detail,
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: c.label.clone(),
                })),
                label: c.label,
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    /// 識別子の文字か `.` を打ったら、補完を出す。
    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        new_text == "."
            || new_text.chars().all(|c| c == '_' || c.is_alphanumeric()) && !new_text.is_empty()
    }
}
