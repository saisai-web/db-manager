//! Excel(.xlsx)の書き出し。大量の行でもメモリを使いすぎないよう、行を順に書き出す。

use std::path::Path;

use rust_xlsxwriter::{Workbook, Worksheet, XlsxError};

use crate::error::IoError;

/// 1 シートの最大行数(見出しを含む)。超えたら、次のシートに続ける。
pub const SHEET_ROW_LIMIT: u32 = 1_048_576;
/// 1 セルの最大文字数
const CELL_CHAR_LIMIT: usize = 32_767;
/// f64 で桁落ちせずに表せる、有効桁数の上限
const EXACT_DIGITS: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Text,
    Number,
    Bool,
}

#[derive(Debug, Clone)]
pub struct XlsxColumn {
    pub name: String,
    pub kind: CellKind,
}

/// 数値として書いても、桁が落ちないか。有効桁数が 15 を超える値は、文字列にする
/// (DB の numeric を Excel の浮動小数点数にして、値を変えてしまわないため)。
fn exact_number(s: &str) -> Option<f64> {
    let digits = s.bytes().filter(u8::is_ascii_digit).count();
    let significant = s
        .trim_start_matches(['-', '+'])
        .trim_start_matches(['0', '.'])
        .bytes()
        .filter(u8::is_ascii_digit)
        .count();
    let plain = s
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'.'));
    if digits == 0 || !plain || significant > EXACT_DIGITS {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn xlsx_err(e: XlsxError) -> IoError {
    IoError::Xlsx(e.to_string())
}

/// `rows` を順に書き出す。`cancel` が真を返したら、書きかけのファイルを残さず中止する。
/// 書き出した行数(見出しを除く)を返す。
pub fn write_xlsx<E: Into<IoError>>(
    path: &Path,
    columns: &[XlsxColumn],
    rows: impl Iterator<Item = Result<Vec<Option<Vec<u8>>>, E>>,
    cancel: impl Fn() -> bool,
) -> Result<u64, IoError> {
    let mut book = Workbook::new();
    let mut sheet = book
        .add_worksheet_with_constant_memory()
        .map_err(xlsx_err)?;
    let mut sheet_no = 1;
    let mut row_in_sheet = 0u32;
    let mut total = 0u64;
    write_header(sheet, columns)?;
    row_in_sheet += 1;

    for row in rows {
        if cancel() {
            return Err(IoError::Cancelled);
        }
        let cells = row.map_err(Into::into)?;
        if row_in_sheet >= SHEET_ROW_LIMIT {
            // 定数メモリのシートは、書き終えると次のシートへ進める
            sheet_no += 1;
            sheet = book
                .add_worksheet_with_constant_memory()
                .map_err(xlsx_err)?;
            sheet
                .set_name(format!("Sheet{sheet_no}"))
                .map_err(xlsx_err)?;
            write_header(sheet, columns)?;
            row_in_sheet = 1;
        }
        total += 1;
        for (col, cell) in cells.iter().enumerate() {
            let Some(bytes) = cell else { continue }; // NULL は空のセル
            let text = String::from_utf8_lossy(bytes);
            let kind = columns.get(col).map_or(CellKind::Text, |c| c.kind);
            write_cell(sheet, row_in_sheet, col as u16, &text, kind).map_err(|e| match e {
                CellError::TooLong => IoError::CellTooLong {
                    row: total,
                    column: columns.get(col).map(|c| c.name.clone()).unwrap_or_default(),
                },
                CellError::Xlsx(e) => xlsx_err(e),
            })?;
        }
        row_in_sheet += 1;
    }
    book.save(path).map_err(xlsx_err)?;
    Ok(total)
}

fn write_header(sheet: &mut Worksheet, columns: &[XlsxColumn]) -> Result<(), IoError> {
    for (i, c) in columns.iter().enumerate() {
        sheet.write_string(0, i as u16, &c.name).map_err(xlsx_err)?;
    }
    Ok(())
}

enum CellError {
    TooLong,
    Xlsx(XlsxError),
}

fn write_cell(
    sheet: &mut Worksheet,
    row: u32,
    col: u16,
    text: &str,
    kind: CellKind,
) -> Result<(), CellError> {
    let r = match kind {
        CellKind::Number => match exact_number(text) {
            Some(n) => sheet.write_number(row, col, n),
            None => return write_text(sheet, row, col, text),
        },
        CellKind::Bool => match text {
            "t" | "true" | "TRUE" | "1" => sheet.write_boolean(row, col, true),
            "f" | "false" | "FALSE" | "0" => sheet.write_boolean(row, col, false),
            _ => return write_text(sheet, row, col, text),
        },
        CellKind::Text => return write_text(sheet, row, col, text),
    };
    r.map(|_| ()).map_err(CellError::Xlsx)
}

fn write_text(sheet: &mut Worksheet, row: u32, col: u16, text: &str) -> Result<(), CellError> {
    if text.chars().count() > CELL_CHAR_LIMIT {
        return Err(CellError::TooLong);
    }
    sheet
        .write_string(row, col, text)
        .map(|_| ())
        .map_err(CellError::Xlsx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn col(name: &str, kind: CellKind) -> XlsxColumn {
        XlsxColumn {
            name: name.into(),
            kind,
        }
    }

    /// xlsx(zip)の中の XML を文字列で読む。
    fn part(path: &Path, name: &str) -> String {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    fn rows(
        v: Vec<Vec<Option<&str>>>,
    ) -> impl Iterator<Item = Result<Vec<Option<Vec<u8>>>, IoError>> {
        v.into_iter().map(|r| {
            Ok(r.into_iter()
                .map(|c| c.map(|s| s.as_bytes().to_vec()))
                .collect())
        })
    }

    #[test]
    fn numbers_are_numeric_only_when_no_digit_is_lost() {
        assert_eq!(exact_number("123.456"), Some(123.456));
        assert_eq!(exact_number("-0.5"), Some(-0.5));
        assert_eq!(exact_number("100000000000000"), Some(1e14), "15 digits");
        assert_eq!(
            exact_number("1234567890123456"),
            None,
            "16 digits would lose precision"
        );
        assert_eq!(exact_number("12345678901234.123456"), None);
        assert_eq!(
            exact_number("0.000000000000001234"),
            Some(0.000000000000001234),
            "leading zeros are not significant"
        );
        assert_eq!(exact_number("NaN"), None);
        assert_eq!(exact_number("1e5"), None);
        assert_eq!(exact_number(""), None);
    }

    #[test]
    fn writes_text_numbers_bools_and_blanks() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let cols = [
            col("名前", CellKind::Text),
            col("金額", CellKind::Number),
            col("有効", CellKind::Bool),
        ];
        let n = write_xlsx(
            f.path(),
            &cols,
            rows(vec![
                vec![Some("佐伯 光平"), Some("12345678901234.123456"), Some("t")],
                vec![None, Some("42"), Some("false")],
            ]),
            || false,
        )
        .unwrap();
        assert_eq!(n, 2);
        let sheet = part(f.path(), "xl/worksheets/sheet1.xml");
        assert!(
            sheet.contains("佐伯 光平") && sheet.contains("名前"),
            "{sheet}"
        );
        // 桁の多い値は、数値にせず、文字列のまま
        assert!(sheet.contains("12345678901234.123456"), "{sheet}");
        assert!(
            sheet.contains("<v>42</v>"),
            "small numbers are numeric: {sheet}"
        );
        assert!(sheet.contains("t=\"b\""), "booleans: {sheet}");
    }

    #[test]
    fn a_too_long_cell_is_an_error_naming_the_row_and_column() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let long = "a".repeat(CELL_CHAR_LIMIT + 1);
        let err = write_xlsx(
            f.path(),
            &[col("memo", CellKind::Text)],
            rows(vec![vec![Some("ok")], vec![Some(&long)]]),
            || false,
        )
        .unwrap_err();
        assert!(
            matches!(err, IoError::CellTooLong { row: 2, ref column } if column == "memo"),
            "{err:?}"
        );
    }

    #[test]
    fn cancel_stops_without_saving() {
        let f = tempfile::tempdir().unwrap();
        let path = f.path().join("out.xlsx");
        let err = write_xlsx(
            &path,
            &[col("a", CellKind::Text)],
            rows(vec![vec![Some("x")]]),
            || true,
        )
        .unwrap_err();
        assert!(matches!(err, IoError::Cancelled));
        assert!(!path.exists());
    }
}
