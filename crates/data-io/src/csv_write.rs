//! CSV の書き出し。NULL と空文字列を区別する(NULL は空、空文字列は `""`)。

use std::io::Write;

use crate::encoding::TextEncoding;
use crate::error::IoError;

#[derive(Debug, Clone)]
pub struct CsvOptions {
    pub encoding: TextEncoding,
    pub delimiter: u8,
    /// NULL の書き方。既定は空。`""`(空文字列)と区別するため、空のときは空文字列を `""` と書く
    pub null_text: String,
    pub header: bool,
    /// 行の終わり。Windows の Excel 向けは `\r\n`
    pub line_ending: &'static str,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            encoding: TextEncoding::Utf8,
            delimiter: b',',
            null_text: String::new(),
            header: true,
            line_ending: "\r\n",
        }
    }
}

pub struct CsvWriter<W: Write> {
    out: W,
    opts: CsvOptions,
    columns: Vec<String>,
    rows: u64,
}

impl<W: Write> CsvWriter<W> {
    /// BOM(必要なら)と、見出し(指定されていれば)を書く。
    pub fn new(mut out: W, opts: CsvOptions, columns: Vec<String>) -> Result<Self, IoError> {
        out.write_all(opts.encoding.bom())?;
        let mut w = Self {
            out,
            opts,
            columns,
            rows: 0,
        };
        if w.opts.header {
            let columns = w.columns.clone();
            let header: Vec<Option<&[u8]>> = columns.iter().map(|c| Some(c.as_bytes())).collect();
            w.write_fields(&header, 0)?;
        }
        Ok(w)
    }

    pub fn rows_written(&self) -> u64 {
        self.rows
    }

    pub fn write_row(&mut self, cells: &[Option<&[u8]>]) -> Result<(), IoError> {
        self.rows += 1;
        self.write_fields(cells, self.rows)
    }

    pub fn finish(mut self) -> Result<W, IoError> {
        self.out.flush()?;
        Ok(self.out)
    }

    fn write_fields(&mut self, cells: &[Option<&[u8]>], row: u64) -> Result<(), IoError> {
        let mut line = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                line.push(self.opts.delimiter as char);
            }
            let text = cell.map(String::from_utf8_lossy);
            self.field(&mut line, text.as_deref());
        }
        line.push_str(self.opts.line_ending);
        let bytes = self.opts.encoding.encode(&line).map_err(|ch| {
            // どの列の値かを示すため、行を 1 列ずつ調べる
            let column = cells
                .iter()
                .position(|c| c.is_some_and(|b| String::from_utf8_lossy(b).contains(ch)))
                .and_then(|i| self.columns.get(i).cloned())
                .unwrap_or_default();
            IoError::Unmappable {
                ch,
                row,
                column,
                encoding: self.opts.encoding,
            }
        })?;
        self.out.write_all(&bytes)?;
        Ok(())
    }

    fn field(&self, line: &mut String, value: Option<&str>) {
        let Some(v) = value else {
            line.push_str(&self.opts.null_text);
            return;
        };
        let d = self.opts.delimiter as char;
        let needs_quotes = v.is_empty() && self.opts.null_text.is_empty()
            || (!self.opts.null_text.is_empty() && v == self.opts.null_text)
            || v.contains([d, '"', '\r', '\n'])
            || v.starts_with(char::is_whitespace)
            || v.ends_with(char::is_whitespace);
        if needs_quotes {
            line.push('"');
            line.push_str(&v.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(
        opts: CsvOptions,
        cols: &[&str],
        rows: &[&[Option<&str>]],
    ) -> Result<Vec<u8>, IoError> {
        let mut w = CsvWriter::new(
            Vec::new(),
            opts,
            cols.iter().map(|s| s.to_string()).collect(),
        )?;
        for r in rows {
            let cells: Vec<Option<&[u8]>> = r.iter().map(|c| c.map(str::as_bytes)).collect();
            w.write_row(&cells)?;
        }
        w.finish()
    }

    fn text(opts: CsvOptions, cols: &[&str], rows: &[&[Option<&str>]]) -> String {
        String::from_utf8(write(opts, cols, rows).unwrap()).unwrap()
    }

    #[test]
    fn null_is_empty_and_the_empty_string_is_quoted() {
        let out = text(
            CsvOptions {
                line_ending: "\n",
                ..Default::default()
            },
            &["a", "b", "c"],
            &[&[None, Some(""), Some("x")]],
        );
        assert_eq!(out, "a,b,c\n,\"\",x\n");
    }

    #[test]
    fn a_custom_null_text_is_used_and_a_matching_string_is_quoted() {
        let opts = CsvOptions {
            null_text: "NULL".into(),
            line_ending: "\n",
            header: false,
            ..Default::default()
        };
        assert_eq!(
            text(
                opts,
                &["a", "b"],
                &[&[None, Some("NULL")], &[Some(""), Some("ok")]]
            ),
            "NULL,\"NULL\"\n,ok\n"
        );
    }

    #[test]
    fn special_characters_are_quoted_and_quotes_doubled() {
        let opts = CsvOptions {
            line_ending: "\n",
            header: false,
            ..Default::default()
        };
        let out = text(
            opts,
            &["a"],
            &[
                &[Some("a,b")],
                &[Some("say \"hi\"")],
                &[Some("line1\nline2")],
                &[Some(" pad ")],
                &[Some("plain")],
            ],
        );
        assert_eq!(
            out,
            "\"a,b\"\n\"say \"\"hi\"\"\"\n\"line1\nline2\"\n\" pad \"\nplain\n"
        );
    }

    #[test]
    fn the_delimiter_can_be_a_tab() {
        let opts = CsvOptions {
            delimiter: b'\t',
            line_ending: "\n",
            header: false,
            ..Default::default()
        };
        assert_eq!(
            text(opts, &["a", "b"], &[&[Some("a,b"), Some("c\td")]]),
            "a,b\t\"c\td\"\n"
        );
    }

    #[test]
    fn utf8_bom_is_written_first() {
        let out = write(
            CsvOptions {
                encoding: TextEncoding::Utf8Bom,
                ..Default::default()
            },
            &["a"],
            &[],
        )
        .unwrap();
        assert!(out.starts_with(&[0xEF, 0xBB, 0xBF]));
    }

    #[test]
    fn shift_jis_output_decodes_back_and_unmappable_characters_are_reported_with_position() {
        let opts = CsvOptions {
            encoding: TextEncoding::ShiftJis,
            line_ending: "\n",
            ..Default::default()
        };
        let out = write(opts.clone(), &["名前"], &[&[Some("佐伯 光平")]]).unwrap();
        assert_eq!(
            crate::decode(&out, TextEncoding::ShiftJis),
            "名前\n佐伯 光平\n"
        );

        let err = write(
            opts,
            &["name", "memo"],
            &[&[Some("ok"), Some("ok")], &[Some("x"), Some("smile 😀")]],
        )
        .unwrap_err();
        let IoError::Unmappable {
            ch, row, column, ..
        } = err
        else {
            panic!("{err:?}")
        };
        assert_eq!((ch, row, column.as_str()), ('😀', 2, "memo"));
    }

    #[test]
    fn rows_are_counted_without_the_header() {
        let mut w = CsvWriter::new(Vec::new(), CsvOptions::default(), vec!["a".into()]).unwrap();
        w.write_row(&[Some(b"1")]).unwrap();
        w.write_row(&[Some(b"2")]).unwrap();
        assert_eq!(w.rows_written(), 2);
    }
}
