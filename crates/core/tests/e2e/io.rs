//! 入出力: 結果の CSV / Excel 書き出しと、CSV の取り込み。

use std::path::{Path, PathBuf};

use sqltool_core::*;

use crate::common::*;

struct Io {
    h: H,
    tab: TabId,
    dir: tempfile::TempDir,
}

impl Io {
    async fn start() -> Option<Self> {
        let db = test_pg::database_or_skip()?;
        let mut h = H::start(db).await;
        let tab = h.open_tab(EnvLabel::Dev).await;
        Some(Self {
            h,
            tab,
            dir: tempfile::tempdir().unwrap(),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    async fn select(&mut self, sql: &str) -> ResultHandle {
        let evs = self.h.exec(self.tab, sql).await;
        result_handle(&evs)
    }

    /// 書き出して、完了(または失敗)のイベントを返す。
    async fn export(&mut self, result: &ResultHandle, path: &Path, format: ExportFormat) -> Event {
        self.h.core.send(Command::ExportResult {
            result: result.id(),
            path: path.to_path_buf(),
            format,
        });
        self.h
            .until(|e| {
                matches!(
                    e,
                    Event::ExportFinished { .. }
                        | Event::CommandFailed {
                            command: "ExportResult",
                            ..
                        }
                )
            })
            .await
            .pop()
            .unwrap()
    }

    async fn import(
        &mut self,
        table: &str,
        path: &Path,
        options: CsvReadOptions,
        mapping: &[(usize, &str)],
    ) -> Event {
        self.h.core.send(Command::ImportCsv {
            tab: self.tab,
            table: TableRef::new("public", table),
            path: path.to_path_buf(),
            options,
            mapping: mapping.iter().map(|(i, n)| (*i, n.to_string())).collect(),
        });
        self.h
            .until(|e| {
                matches!(
                    e,
                    Event::ImportFinished { .. }
                        | Event::ImportFailed { .. }
                        | Event::CommandFailed {
                            command: "ImportCsv",
                            ..
                        }
                )
            })
            .await
            .pop()
            .unwrap()
    }

    async fn count(&mut self, table: &str) -> String {
        let r = self.select(&format!("select count(*) from {table}")).await;
        String::from_utf8(r.read_row_blocking(0).unwrap().unwrap()[0].clone().unwrap()).unwrap()
    }
}

const SAMPLE: &str = "select * from (values
  (1, 'Alice', 12345678901234.123456::numeric, 'a,b'::text, true, '\\xdead'::bytea),
  (2, null, 0.5, '', false, null),
  (3, '佐伯 光平', -1, 'say \"hi\"', null, '\\x00'::bytea)
) t(id, name, amount, note, flag, bin) order by id";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn csv_export_keeps_null_distinct_from_empty_and_values_exact() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    let r = io.select(SAMPLE).await;
    let path = io.path("out.csv");
    let opts = CsvOptions {
        line_ending: "\n",
        ..Default::default()
    };
    let done = io.export(&r, &path, ExportFormat::Csv(opts)).await;
    assert!(
        matches!(
            done,
            Event::ExportFinished {
                rows: 3,
                partial: false,
                ..
            }
        ),
        "{done:?}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "id,name,amount,note,flag,bin\n\
         1,Alice,12345678901234.123456,\"a,b\",t,\\xdead\n\
         2,,0.5,\"\",f,\n\
         3,佐伯 光平,-1,\"say \"\"hi\"\"\",,\\x00\n"
    );
    assert!(
        !io.dir.path().join(".out.csv.part").exists(),
        "no temporary file is left"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_export_follows_the_displayed_sort_order() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    let r = io.select("select g from generate_series(1, 5) g").await;
    io.h.core.send(Command::SortResult {
        result: r.id(),
        column: 0,
        order: SortOrder::Descending,
    });
    io.h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    let path = io.path("sorted.csv");
    io.export(
        &r,
        &path,
        ExportFormat::Csv(CsvOptions {
            header: false,
            line_ending: "\n",
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(std::fs::read_to_string(path).unwrap(), "5\n4\n3\n2\n1\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shift_jis_and_bom_variants_and_the_unmappable_character_error() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    let r = io.select("select '日本語' as 名前, 'ABC' as b").await;
    let sjis = io.path("sjis.csv");
    io.export(
        &r,
        &sjis,
        ExportFormat::Csv(CsvOptions {
            encoding: TextEncoding::ShiftJis,
            line_ending: "\n",
            ..Default::default()
        }),
    )
    .await;
    let bytes = std::fs::read(&sjis).unwrap();
    assert!(std::str::from_utf8(&bytes).is_err(), "not UTF-8");
    assert_eq!(
        data_io::decode(&bytes, data_io::TextEncoding::ShiftJis),
        "名前,b\n日本語,ABC\n"
    );

    let bom = io.path("bom.csv");
    io.export(
        &r,
        &bom,
        ExportFormat::Csv(CsvOptions {
            encoding: TextEncoding::Utf8Bom,
            ..Default::default()
        }),
    )
    .await;
    assert!(
        std::fs::read(&bom)
            .unwrap()
            .starts_with(&[0xEF, 0xBB, 0xBF])
    );

    // Shift_JIS で表せない文字は、置き換えずにエラーにする。書きかけのファイルも残さない
    let emoji = io.select("select 'ok' as a, 'smile 😀' as memo").await;
    let bad = io.path("bad.csv");
    let done = io
        .export(
            &emoji,
            &bad,
            ExportFormat::Csv(CsvOptions {
                encoding: TextEncoding::ShiftJis,
                ..Default::default()
            }),
        )
        .await;
    let Event::CommandFailed { error, .. } = done else {
        panic!("{done:?}")
    };
    assert!(
        error.message.contains('😀') && error.message.contains("memo"),
        "{error}"
    );
    assert!(!bad.exists() && !io.dir.path().join(".bad.csv.part").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn excel_export_writes_a_valid_workbook_and_a_partial_result_is_flagged() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    let r = io.select(SAMPLE).await;
    let path = io.path("out.xlsx");
    let done = io.export(&r, &path, ExportFormat::Xlsx).await;
    assert!(
        matches!(
            done,
            Event::ExportFinished {
                rows: 3,
                partial: false,
                ..
            }
        ),
        "{done:?}"
    );
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"PK"), "an xlsx is a zip file");

    // 取得上限で止まっている結果は、全件ではないと知らせる
    io.h.send_exec(
        io.tab,
        "select g from generate_series(1, 5000) g",
        ExecuteTarget::All,
        ExecuteOptions {
            fetch_limit: FetchLimit::Rows(100),
            ..Default::default()
        },
    );
    let evs = io.h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let paused = result_handle(&evs);
    let part = io.path("partial.csv");
    let done = io
        .export(&paused, &part, ExportFormat::Csv(CsvOptions::default()))
        .await;
    assert!(
        matches!(
            done,
            Event::ExportFinished {
                rows: 100,
                partial: true,
                ..
            }
        ),
        "{done:?}"
    );
    io.h.core.send(Command::Cancel { tab: io.tab });
    io.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
}

const SCHEMA: &str = "create table items (id bigint primary key, name text, price numeric(20,6), added timestamptz, ok boolean)";

fn write_csv(path: &Path, rows: usize) {
    let mut s = String::from("id,name,price,added,ok\n");
    for i in 1..=rows {
        let name = if i % 10 == 0 {
            String::new()
        } else {
            format!("商品{i},\"x\"")
        };
        s.push_str(&format!(
            "{i},\"{}\",{}.123456,2026-11-02 12:34:56+09,{}\n",
            name.replace('"', "\"\""),
            i * 3,
            i % 2 == 0
        ));
    }
    std::fs::write(path, s).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn importing_a_csv_loads_every_row_in_batches_with_exact_values() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    io.h.exec(io.tab, SCHEMA).await;
    let csv = io.path("items.csv");
    write_csv(&csv, 1_234); // 500 行ずつ、3 回に分かれる
    let done = io
        .import(
            "items",
            &csv,
            CsvReadOptions::default(),
            &[
                (0, "id"),
                (1, "name"),
                (2, "price"),
                (3, "added"),
                (4, "ok"),
            ],
        )
        .await;
    assert!(
        matches!(done, Event::ImportFinished { rows: 1_234, .. }),
        "{done:?}"
    );
    assert_eq!(io.count("items").await, "1234");
    let r = io
        .select("select name, price, ok from items where id in (9, 10, 1234) order by id")
        .await;
    let cell = |row: u64, col: usize| {
        r.read_row_blocking(row).unwrap().unwrap()[col]
            .clone()
            .map(|b| String::from_utf8(b).unwrap())
    };
    assert_eq!(cell(0, 0).as_deref(), Some("商品9,\"x\""));
    assert_eq!(cell(1, 0), None, "an empty CSV field is NULL");
    assert_eq!(cell(2, 1).as_deref(), Some("3702.123456"));
    assert_eq!(cell(0, 2).as_deref(), Some("f"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_row_rolls_back_the_whole_import_and_says_where() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    io.h.exec(io.tab, SCHEMA).await;
    let csv = io.path("bad.csv");
    let mut s = String::from("id,name,price\n");
    for i in 1..=900 {
        s.push_str(&format!("{i},n{i},1.5\n"));
    }
    s.push_str("901,n901,not-a-number\n");
    std::fs::write(&csv, s).unwrap();
    let done = io
        .import(
            "items",
            &csv,
            CsvReadOptions::default(),
            &[(0, "id"), (1, "name"), (2, "price")],
        )
        .await;
    let Event::ImportFailed { error, .. } = done else {
        panic!("{done:?}")
    };
    assert_eq!(error.code.as_deref(), Some("22P02"), "{error:?}");
    assert!(
        error.message.contains("501 〜 900 行目") || error.message.contains("行目"),
        "{}",
        error.message
    );
    assert_eq!(
        io.count("items").await,
        "0",
        "nothing is imported on failure"
    );
    assert!(io.h.core.pending_transactions().is_empty());
    // 取り消した後も、タブは使える
    assert_eq!(io.count("items").await, "0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shift_jis_and_bom_csv_files_are_detected_and_the_preview_describes_them() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    io.h.exec(io.tab, "create table people (name text, memo text)")
        .await;
    let sjis = io.path("people.csv");
    std::fs::write(
        &sjis,
        data_io::TextEncoding::ShiftJis
            .encode("name\tmemo\n佐伯 光平\t日本語のメモ\n")
            .unwrap(),
    )
    .unwrap();
    io.h.core.send(Command::PreviewCsv {
        path: sjis.clone(),
        options: CsvReadOptions::default(),
    });
    let evs = io.h.until(|e| matches!(e, Event::CsvPreview { .. })).await;
    let Event::CsvPreview {
        encoding,
        delimiter,
        headers,
        rows,
        ..
    } = evs.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!((*encoding, *delimiter), (TextEncoding::ShiftJis, b'\t'));
    assert_eq!(headers, &["name", "memo"]);
    assert_eq!(rows[0][0].as_deref(), Some("佐伯 光平"));

    let done = io
        .import(
            "people",
            &sjis,
            CsvReadOptions::default(),
            &[(0, "name"), (1, "memo")],
        )
        .await;
    assert!(
        matches!(done, Event::ImportFinished { rows: 1, .. }),
        "{done:?}"
    );
    let r = io.select("select memo from people").await;
    assert_eq!(
        r.read_row_blocking(0).unwrap().unwrap()[0].as_deref(),
        Some("日本語のメモ".as_bytes())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsafe_imports_are_refused() {
    let Some(mut io) = Io::start().await else {
        return;
    };
    io.h.exec(io.tab, SCHEMA).await;
    let csv = io.path("a.csv");
    std::fs::write(&csv, "id,name\n1,x\n").unwrap();
    // 存在しない列、空の対応づけ
    let r = io
        .import("items", &csv, CsvReadOptions::default(), &[(0, "nope")])
        .await;
    assert!(
        matches!(
            r,
            Event::CommandFailed {
                command: "ImportCsv",
                ..
            }
        ),
        "{r:?}"
    );
    let r = io
        .import("items", &csv, CsvReadOptions::default(), &[])
        .await;
    assert!(
        matches!(
            r,
            Event::CommandFailed {
                command: "ImportCsv",
                ..
            }
        ),
        "{r:?}"
    );
    // 読み取り専用モード
    io.h.core.send(Command::SetReadOnly {
        tab: io.tab,
        read_only: true,
    });
    io.h.until(|e| matches!(e, Event::ReadOnlyChanged { .. }))
        .await;
    let r = io
        .import("items", &csv, CsvReadOptions::default(), &[(0, "id")])
        .await;
    assert!(
        matches!(
            r,
            Event::CommandFailed {
                command: "ImportCsv",
                ..
            }
        ),
        "{r:?}"
    );
    io.h.core.send(Command::SetReadOnly {
        tab: io.tab,
        read_only: false,
    });
    io.h.until(|e| matches!(e, Event::ReadOnlyChanged { .. }))
        .await;
    assert_eq!(io.count("items").await, "0");
}
