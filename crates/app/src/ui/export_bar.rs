//! 結果の書き出しボタン。保存先は OS の保存ダイアログで選ぶ。

use std::path::PathBuf;

use sqltool_core::{CsvOptions, ExportFormat, ResultId, TabId, TextEncoding};

use super::prelude::*;
use super::workspace::Workspace;

/// ボタンの表示名、保存ファイルの既定名、書式。Windows の Excel 向けに、BOM 付きと Shift_JIS も選べる。
fn formats() -> [(&'static str, &'static str, ExportFormat); 4] {
    let csv = |encoding| {
        ExportFormat::Csv(CsvOptions {
            encoding,
            ..Default::default()
        })
    };
    [
        ("CSV", "result.csv", csv(TextEncoding::Utf8)),
        ("CSV(BOM 付き)", "result.csv", csv(TextEncoding::Utf8Bom)),
        ("CSV(Shift_JIS)", "result.csv", csv(TextEncoding::ShiftJis)),
        ("Excel", "result.xlsx", ExportFormat::Xlsx),
    ]
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn export_buttons(
    tab: TabId,
    result: ResultId,
    cx: &mut Context<Workspace>,
) -> impl IntoElement + use<> {
    let buttons: Vec<_> = formats()
        .into_iter()
        .enumerate()
        .map(|(ix, (label, name, format))| {
            Button::new(("export", ix))
                .label(label)
                .ghost()
                .small()
                .on_click(cx.listener(move |_, _, _, cx| {
                    let rx = cx.prompt_for_new_path(&home(), Some(name));
                    let format = format.clone();
                    cx.spawn(async move |this, cx| {
                        let Ok(Ok(Some(path))) = rx.await else { return };
                        let _ = this.update(cx, |this, cx| {
                            this.model
                                .update(cx, |m, _| m.export(tab, result, path, format))
                        });
                    })
                    .detach();
                }))
        })
        .collect();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(div().text_xs().child("書き出し:"))
        .children(buttons)
}
