//! ファイルへの書き出しと、CSV の取り込み。

use std::path::PathBuf;

use gpui_kit::Context;
use sqltool_core::{
    Command, CsvReadOptions, Event, ExportFormat, ResultId, TabId, TableRef, TextEncoding,
};

use super::AppModel;
use super::status::StatusLine;

/// 取り込み前の確認(プレビュー)。
pub struct ImportPreviewState {
    pub table: TableRef,
    pub path: PathBuf,
    pub encoding: TextEncoding,
    pub delimiter: u8,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub has_header: bool,
}

/// CSV の列を、テーブルの列に対応づける。見出しがあれば列名(大文字小文字を区別しない)で、
/// なければ先頭から順に。対応する列がない CSV の列は、取り込まない。
pub fn default_mapping(
    csv_columns: &[String],
    has_header: bool,
    table_columns: &[String],
) -> Vec<(usize, String)> {
    if !has_header {
        return table_columns
            .iter()
            .take(csv_columns.len())
            .enumerate()
            .map(|(i, c)| (i, c.clone()))
            .collect();
    }
    csv_columns
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            table_columns
                .iter()
                .find(|c| c.eq_ignore_ascii_case(h))
                .map(|c| (i, c.clone()))
        })
        .collect()
}

fn read_options(has_header: bool) -> CsvReadOptions {
    CsvReadOptions {
        has_header,
        ..Default::default()
    }
}

impl AppModel {
    pub fn export(&mut self, tab: TabId, result: ResultId, path: PathBuf, format: ExportFormat) {
        self.set_status(
            tab,
            StatusLine::info(format!("書き出し中… {}", path.display())),
        );
        self.core.send(Command::ExportResult {
            result,
            path,
            format,
        });
    }

    /// 取り込む CSV を選んだ。内容を確かめるためにプレビューを求め、列の対応づけに使う表の列も読み込む。
    pub fn start_import(&mut self, tab: TabId, table: TableRef, path: PathBuf) {
        let Some(t) = self.tab_mut(tab) else { return };
        let connection_id = t.connection.id.clone();
        t.import_preview = Some(ImportPreviewState {
            table: table.clone(),
            path: path.clone(),
            encoding: TextEncoding::Utf8,
            delimiter: b',',
            headers: Vec::new(),
            rows: Vec::new(),
            has_header: true,
        });
        self.core.send(Command::LoadTableDetails {
            connection_id,
            table,
        });
        self.core.send(Command::PreviewCsv {
            path,
            options: read_options(true),
        });
    }

    pub fn set_import_header(&mut self, tab: TabId, has_header: bool, cx: &mut Context<Self>) {
        let Some(p) = self.tab_mut(tab).and_then(|t| t.import_preview.as_mut()) else {
            return;
        };
        p.has_header = has_header;
        let path = p.path.clone();
        self.core.send(Command::PreviewCsv {
            path,
            options: read_options(has_header),
        });
        cx.notify();
    }

    /// 表の列(読み込み済みなら)。
    pub fn import_table_columns(&self, tab: TabId) -> Option<Vec<String>> {
        let t = self.tab(tab)?;
        let p = t.import_preview.as_ref()?;
        let d = self
            .schemas
            .get(&t.connection.id)?
            .details
            .get(&(p.table.schema.clone(), p.table.name.clone()))?;
        Some(d.columns.iter().map(|c| c.name.clone()).collect())
    }

    pub fn confirm_import(&mut self, tab: TabId) {
        let Some(table_columns) = self.import_table_columns(tab) else {
            return;
        };
        let Some(p) = self.tab_mut(tab).and_then(|t| t.import_preview.take()) else {
            return;
        };
        let mapping = default_mapping(&p.headers, p.has_header, &table_columns);
        self.set_status(tab, StatusLine::info("取り込み中…"));
        self.core.send(Command::ImportCsv {
            tab,
            table: p.table,
            path: p.path,
            options: read_options(p.has_header),
            mapping,
        });
    }

    pub fn cancel_import(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(tab) {
            t.import_preview = None;
        }
        cx.notify();
    }

    pub(super) fn apply_io_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::ExportProgress { result, rows } => {
                if let Some(tab) = self.tab_of_result(result) {
                    self.set_status(tab, StatusLine::info(format!("書き出し中… {rows} 行")));
                }
            }
            Event::ExportFinished {
                result,
                path,
                rows,
                partial,
            } => {
                if let Some(tab) = self.tab_of_result(result) {
                    let text = format!("{rows} 行を書き出しました: {}", path.display());
                    let line = if partial {
                        StatusLine::warn(format!(
                            "{text}(結果の一部だけです。全件を受信していません)"
                        ))
                    } else {
                        StatusLine::ok(text)
                    };
                    self.set_status(tab, line);
                }
            }
            Event::CsvPreview {
                path,
                encoding,
                delimiter,
                headers,
                rows,
            } => {
                for t in &mut self.tabs {
                    if let Some(p) = t.import_preview.as_mut().filter(|p| p.path == path) {
                        p.encoding = encoding;
                        p.delimiter = delimiter;
                        p.headers = headers.clone();
                        p.rows = rows.clone();
                    }
                }
            }
            Event::ImportProgress { tab, rows } => {
                self.set_status(tab, StatusLine::info(format!("取り込み中… {rows} 行")));
            }
            Event::ImportFinished { tab, rows } => {
                self.set_status(tab, StatusLine::ok(format!("{rows} 行を取り込みました")));
            }
            Event::ImportFailed { tab, error } => {
                self.set_status(
                    tab,
                    StatusLine::error(format!(
                        "取り込めませんでした(すべて取り消しました): {error}"
                    )),
                );
            }
            _ => {}
        }
        cx.notify();
    }

    fn tab_of_result(&self, result: ResultId) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|t| t.results.iter().any(|r| r.handle.id() == result))
            .map(|t| t.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn headers_are_matched_to_columns_ignoring_case_and_unknown_columns_are_skipped() {
        let m = default_mapping(
            &v(&["ID", "Name", "extra", "price"]),
            true,
            &v(&["id", "name", "price"]),
        );
        assert_eq!(
            m,
            [(0, "id".into()), (1, "name".into()), (3, "price".into())]
        );
    }

    #[test]
    fn without_a_header_columns_are_matched_by_position() {
        let m = default_mapping(&v(&["列1", "列2"]), false, &v(&["a", "b", "c"]));
        assert_eq!(m, [(0, "a".into()), (1, "b".into())]);
        // CSV の列のほうが多ければ、余りは取り込まない
        let m = default_mapping(&v(&["列1", "列2", "列3"]), false, &v(&["a"]));
        assert_eq!(m, [(0, "a".into())]);
    }

    #[test]
    fn nothing_matches_gives_an_empty_mapping() {
        assert!(default_mapping(&v(&["x"]), true, &v(&["a"])).is_empty());
    }
}
