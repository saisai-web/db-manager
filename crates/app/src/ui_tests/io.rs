//! ファイルの入出力の画面操作: CSV の取り込み(プレビュー → 確定)と、結果の書き出し。

use gpui_kit::TestAppContext;
use sqltool_core::{CsvOptions, ExportFormat, TableRef};

use super::harness::*;

fn status_contains(cx: &mut TestAppContext, f: &Fixture, text: &str) -> bool {
    cx.update(|cx| {
        f.model
            .read(cx)
            .active_tab()
            .is_some_and(|t| t.status.text.contains(text))
    })
}

fn first_cell(cx: &mut TestAppContext, f: &Fixture) -> String {
    cx.update(|cx| {
        let r = f
            .model
            .read(cx)
            .active_tab()
            .unwrap()
            .active_result()
            .unwrap();
        String::from_utf8(
            r.handle.read_row_blocking(0).unwrap().unwrap()[0]
                .clone()
                .unwrap(),
        )
        .unwrap()
    })
}

#[gpui_kit::test]
fn importing_a_csv_goes_through_preview_and_loads_the_rows(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "create table people (id int, name text)");
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("people.csv");
    std::fs::write(&csv, "ID,Name,ignored\n1,佐伯,x\n2,,y\n").unwrap();

    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, _| {
            m.start_import(tab, TableRef::new("public", "people"), csv.clone())
        });
    });
    // プレビュー(見出しと行)と、表の列が揃うのを待つ
    wait_until(cx, "the preview and the table columns", |cx| {
        cx.update(|cx| {
            let m = f.model.read(cx);
            let tab = m.active.unwrap();
            m.active_tab()
                .and_then(|t| t.import_preview.as_ref())
                .is_some_and(|p| p.headers.len() == 3 && p.rows.len() == 2)
                && m.import_table_columns(tab).is_some()
        })
    });
    cx.update(|cx| {
        let p = f
            .model
            .read(cx)
            .active_tab()
            .unwrap()
            .import_preview
            .as_ref()
            .unwrap();
        assert_eq!(p.encoding, sqltool_core::TextEncoding::Utf8);
        assert_eq!(p.delimiter, b',');
    });

    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, _| m.confirm_import(tab));
    });
    wait_until(cx, "the import to finish", |cx| {
        status_contains(cx, &f, "2 行を取り込みました")
    });
    run_sql(cx, &f, "select count(*) from people where name is null");
    assert_eq!(first_cell(cx, &f), "1", "the empty CSV field became NULL");
}

#[gpui_kit::test]
fn cancelling_the_import_preview_loads_nothing(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "create table t (a int)");
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("t.csv");
    std::fs::write(&csv, "a\n1\n").unwrap();
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, _| {
            m.start_import(tab, TableRef::new("public", "t"), csv.clone())
        });
    });
    wait_until(cx, "the preview", |cx| {
        cx.update(|cx| {
            f.model
                .read(cx)
                .active_tab()
                .and_then(|t| t.import_preview.as_ref())
                .is_some_and(|p| !p.headers.is_empty())
        })
    });
    cx.update(|cx| {
        let tab = f.model.read(cx).active.unwrap();
        f.model.update(cx, |m, cx| m.cancel_import(tab, cx));
    });
    run_sql(cx, &f, "select count(*) from t");
    assert_eq!(first_cell(cx, &f), "0");
}

#[gpui_kit::test]
fn exporting_a_result_writes_the_file_and_reports_it(cx: &mut TestAppContext) {
    let Some(f) = open(cx, false) else { return };
    wait_for_tab(cx, &f);
    run_sql(cx, &f, "select 1 as a, 'x' as b");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.csv");
    cx.update(|cx| {
        let t = f.model.read(cx).active_tab().unwrap();
        let (tab, result) = (t.id, t.active_result().unwrap().handle.id());
        let format = ExportFormat::Csv(CsvOptions {
            line_ending: "\n",
            ..Default::default()
        });
        f.model
            .update(cx, |m, _| m.export(tab, result, path.clone(), format));
    });
    wait_until(cx, "the export to finish", |cx| {
        status_contains(cx, &f, "1 行を書き出しました")
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a,b\n1,x\n");
}
