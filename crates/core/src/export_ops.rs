//! 結果のファイルへの書き出し(CSV / Excel)。別のスレッドで、進捗を知らせながら行う。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use data_io::{CellKind, CsvWriter, IoError, XlsxColumn, write_xlsx};

use crate::core::{Res, Shared, lock};
use crate::result::ResultHandle;
use crate::types::*;

/// 進捗を知らせる行数の間隔
const PROGRESS_EVERY: u64 = 10_000;

/// バイナリは、PostgreSQL の入力形式(`\x` + 16 進数)で書く。
fn hex_text(bytes: &[u8]) -> Vec<u8> {
    let mut out = b"\\x".to_vec();
    for b in bytes {
        out.extend_from_slice(format!("{b:02x}").as_bytes());
    }
    out
}

impl Shared {
    pub(crate) fn export_result(
        self: &Arc<Self>,
        result: ResultId,
        path: PathBuf,
        format: ExportFormat,
    ) -> Res {
        let handle = self.result_handle(result)?;
        if lock(&self.exports).contains_key(&result) {
            return Err(UserError::new("この結果は書き出し中です"));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.exports).insert(result, Arc::clone(&cancel));
        let sh = Arc::clone(self);
        // 結果の全件を受信していなければ、書き出すのは、いま受信済みの分だけ
        let partial = !(handle.is_finished() && !handle.is_truncated());
        self.rt.spawn_blocking(move || {
            let events = sh.events.clone();
            let progress = |rows| {
                let _ = events.send(Event::ExportProgress { result, rows });
            };
            let outcome = write_atomically(&path, |tmp| {
                write_rows(&handle, tmp, &format, &cancel, &progress)
            });
            lock(&sh.exports).remove(&result);
            sh.emit(match outcome {
                Ok(rows) => Event::ExportFinished {
                    result,
                    path,
                    rows,
                    partial,
                },
                Err(e) => Event::CommandFailed {
                    command: "ExportResult",
                    tab: Some(handle.tab()),
                    error: UserError::new(e.to_string()),
                },
            });
        });
        Ok(())
    }
}

/// 書きかけのファイルが、目的のファイルを壊さないよう、一時ファイルに書いてから置き換える。
fn write_atomically(
    path: &Path,
    write: impl FnOnce(&Path) -> Result<u64, IoError>,
) -> Result<u64, IoError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.part"));
    let result = write(&tmp).and_then(|rows| {
        std::fs::rename(&tmp, path)?;
        Ok(rows)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 画面の並び順で、行を 1 行ずつ読む。
fn rows_of<'a>(
    handle: &'a ResultHandle,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(u64),
) -> impl Iterator<Item = Result<Vec<Option<Vec<u8>>>, IoError>> + 'a {
    let binary: Vec<bool> = handle
        .columns()
        .iter()
        .map(|c| c.class == ValueClass::Binary)
        .collect();
    (0..handle.row_count()).map_while(move |pos| {
        if cancel.load(Ordering::Acquire) {
            return Some(Err(IoError::Cancelled));
        }
        if pos > 0 && pos % PROGRESS_EVERY == 0 {
            progress(pos);
        }
        let row = handle.read_row_blocking(pos);
        Some(match row {
            Ok(Some(mut cells)) => {
                for (cell, is_bin) in cells.iter_mut().zip(&binary) {
                    if *is_bin {
                        *cell = cell.as_ref().map(|b| hex_text(b));
                    }
                }
                Ok(cells)
            }
            Ok(None) => Err(IoError::Io(std::io::Error::other(
                "行を読み出せませんでした",
            ))),
            Err(e) => Err(IoError::Io(std::io::Error::other(e.to_string()))),
        })
    })
}

fn write_rows(
    handle: &ResultHandle,
    tmp: &Path,
    format: &ExportFormat,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<u64, IoError> {
    let columns: Vec<String> = handle.columns().iter().map(|c| c.name.clone()).collect();
    match format {
        ExportFormat::Csv(opts) => {
            let file = BufWriter::new(File::create(tmp)?);
            let mut w = CsvWriter::new(file, opts.clone(), columns)?;
            for row in rows_of(handle, cancel, progress) {
                let cells = row?;
                let refs: Vec<Option<&[u8]>> = cells.iter().map(|c| c.as_deref()).collect();
                w.write_row(&refs)?;
            }
            let rows = w.rows_written();
            w.finish()?.flush()?;
            Ok(rows)
        }
        ExportFormat::Xlsx => {
            let cols: Vec<XlsxColumn> = handle
                .columns()
                .iter()
                .map(|c| XlsxColumn {
                    name: c.name.clone(),
                    kind: match c.class {
                        ValueClass::Number => CellKind::Number,
                        ValueClass::Bool => CellKind::Bool,
                        _ => CellKind::Text,
                    },
                })
                .collect();
            write_xlsx(tmp, &cols, rows_of(handle, cancel, progress), || {
                cancel.load(Ordering::Acquire)
            })
        }
    }
}
