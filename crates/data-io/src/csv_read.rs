//! CSV の読み取り。文字コードと区切り文字は、指定がなければファイルから推定する。
//! 大きなファイルでも、全体をメモリに載せず、1 行ずつ読む。

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use encoding_rs::{Decoder, SHIFT_JIS};

use crate::encoding::{TextEncoding, UTF8_BOM, decode, detect};
use crate::error::IoError;

#[derive(Debug, Clone)]
pub struct ReadOptions {
    pub delimiter: Option<u8>,
    pub encoding: Option<TextEncoding>,
    /// 1 行目が見出し
    pub has_header: bool,
    /// 空のフィールドを NULL とする(しなければ空文字列)
    pub empty_as_null: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            delimiter: None,
            encoding: None,
            has_header: true,
            empty_as_null: true,
        }
    }
}

/// 先頭の一部。文字コードと区切り文字の推定に使う。
const SNIFF_BYTES: usize = 64 * 1024;

/// Shift_JIS を UTF-8 に変換しながら読む。UTF-8 は、BOM だけ取り除く。
struct Utf8Reader<R: Read> {
    inner: R,
    decoder: Option<Decoder>,
    raw: Vec<u8>,
    out: Vec<u8>,
    out_pos: usize,
    started: bool,
    eof: bool,
}

impl<R: Read> Utf8Reader<R> {
    fn new(inner: R, enc: TextEncoding) -> Self {
        Self {
            inner,
            decoder: (enc == TextEncoding::ShiftJis)
                .then(|| SHIFT_JIS.new_decoder_without_bom_handling()),
            raw: vec![0; 32 * 1024],
            out: Vec::new(),
            out_pos: 0,
            started: false,
            eof: false,
        }
    }

    fn fill(&mut self) -> io::Result<()> {
        let n = self.inner.read(&mut self.raw)?;
        let mut chunk = &self.raw[..n];
        if !self.started {
            self.started = true;
            if self.decoder.is_none() && chunk.starts_with(UTF8_BOM) {
                chunk = &chunk[UTF8_BOM.len()..];
            }
        }
        self.out.clear();
        self.out_pos = 0;
        match &mut self.decoder {
            Some(d) => {
                // 変換後の長さの上限で確保するので、1 回の呼び出しで、入力をすべて変換できる
                let cap = d
                    .max_utf8_buffer_length(chunk.len())
                    .unwrap_or(chunk.len() * 3 + 16);
                self.out.resize(cap, 0);
                let (_, _, written, _) = d.decode_to_utf8(chunk, &mut self.out, n == 0);
                self.out.truncate(written);
            }
            None => self.out.extend_from_slice(chunk),
        }
        self.eof = n == 0;
        Ok(())
    }
}

impl<R: Read> Read for Utf8Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.out_pos >= self.out.len() {
            if self.eof {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = buf.len().min(self.out.len() - self.out_pos);
        buf[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
        self.out_pos += n;
        Ok(n)
    }
}

pub struct CsvReader {
    pub encoding: TextEncoding,
    pub delimiter: u8,
    headers: Vec<String>,
    reader: csv::Reader<Utf8Reader<File>>,
    pending: Option<Vec<Option<String>>>,
    empty_as_null: bool,
    line: u64,
}

/// 見出しと、先頭の数行(取り込み前の確認用)。
pub struct CsvData {
    pub encoding: TextEncoding,
    pub delimiter: u8,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

fn detect_delimiter(head: &str) -> u8 {
    let first_line = head.lines().next().unwrap_or("");
    b",\t;|"
        .iter()
        .copied()
        .max_by_key(|&d| first_line.bytes().filter(|&b| b == d).count())
        .filter(|&d| first_line.bytes().any(|b| b == d))
        .unwrap_or(b',')
}

impl CsvReader {
    pub fn open(path: &Path, opts: &ReadOptions) -> Result<Self, IoError> {
        let mut head = Vec::with_capacity(SNIFF_BYTES);
        File::open(path)?
            .take(SNIFF_BYTES as u64)
            .read_to_end(&mut head)?;
        let encoding = opts.encoding.unwrap_or_else(|| detect(&head));
        let delimiter = opts
            .delimiter
            .unwrap_or_else(|| detect_delimiter(&decode(&head, encoding)));

        let reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(false)
            .from_reader(Utf8Reader::new(File::open(path)?, encoding));
        let mut me = Self {
            encoding,
            delimiter,
            headers: Vec::new(),
            reader,
            pending: None,
            empty_as_null: opts.empty_as_null,
            line: 0,
        };
        let first = me.read_record()?;
        match first {
            Some(rec) if opts.has_header => {
                me.headers = rec.into_iter().map(Option::unwrap_or_default).collect();
            }
            Some(rec) => {
                me.headers = (1..=rec.len()).map(|i| format!("列{i}")).collect();
                me.pending = Some(rec);
            }
            None => {}
        }
        Ok(me)
    }

    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    /// 次の行。見出しの後の最初の行から順に返す。
    pub fn next_record(&mut self) -> Result<Option<Vec<Option<String>>>, IoError> {
        if let Some(r) = self.pending.take() {
            return Ok(Some(r));
        }
        self.read_record()
    }

    fn read_record(&mut self) -> Result<Option<Vec<Option<String>>>, IoError> {
        let mut rec = csv::ByteRecord::new();
        let more = self
            .reader
            .read_byte_record(&mut rec)
            .map_err(|e| IoError::Csv(e.to_string()))?;
        if !more {
            return Ok(None);
        }
        self.line += 1;
        Ok(Some(
            rec.iter()
                .map(|f| {
                    let s = String::from_utf8_lossy(f).into_owned();
                    (!(s.is_empty() && self.empty_as_null)).then_some(s)
                })
                .collect(),
        ))
    }

    /// 先頭の `n` 行(確認用)を読む。
    pub fn preview(path: &Path, opts: &ReadOptions, n: usize) -> Result<CsvData, IoError> {
        let mut r = Self::open(path, opts)?;
        let mut rows = Vec::new();
        while rows.len() < n {
            match r.next_record()? {
                Some(rec) => rows.push(rec),
                None => break,
            }
        }
        Ok(CsvData {
            encoding: r.encoding,
            delimiter: r.delimiter,
            headers: r.headers.clone(),
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f
    }

    fn read_all(
        f: &tempfile::NamedTempFile,
        opts: &ReadOptions,
    ) -> (CsvReader, Vec<Vec<Option<String>>>) {
        let mut r = CsvReader::open(f.path(), opts).unwrap();
        let mut rows = Vec::new();
        while let Some(rec) = r.next_record().unwrap() {
            rows.push(rec);
        }
        (r, rows)
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn reads_headers_quotes_embedded_newlines_and_empty_fields() {
        let f = file(b"id,name,note\n1,\"Smith, J\",\"line1\nline2\"\n2,,\"\"\n");
        let (r, rows) = read_all(&f, &ReadOptions::default());
        assert_eq!(r.headers(), ["id", "name", "note"]);
        assert_eq!(rows[0], [s("1"), s("Smith, J"), s("line1\nline2")]);
        assert_eq!(
            rows[1],
            [s("2"), None, None],
            "empty fields become NULL by default"
        );
    }

    #[test]
    fn empty_fields_can_stay_empty_strings() {
        let f = file(b"a,b\n,x\n");
        let (_, rows) = read_all(
            &f,
            &ReadOptions {
                empty_as_null: false,
                ..Default::default()
            },
        );
        assert_eq!(rows[0], [s(""), s("x")]);
    }

    #[test]
    fn without_a_header_columns_are_numbered_and_the_first_row_is_data() {
        let f = file(b"1,2\n3,4\n");
        let (r, rows) = read_all(
            &f,
            &ReadOptions {
                has_header: false,
                ..Default::default()
            },
        );
        assert_eq!(r.headers(), ["列1", "列2"]);
        assert_eq!(rows, vec![vec![s("1"), s("2")], vec![s("3"), s("4")]]);
    }

    #[test]
    fn the_delimiter_and_a_bom_are_detected() {
        let f = file(b"\xEF\xBB\xBFa\tb\n1\t2\n");
        let (r, rows) = read_all(&f, &ReadOptions::default());
        assert_eq!((r.encoding, r.delimiter), (TextEncoding::Utf8Bom, b'\t'));
        assert_eq!(
            r.headers(),
            ["a", "b"],
            "the BOM must not end up in the first header"
        );
        assert_eq!(rows[0], [s("1"), s("2")]);
        let f = file(b"a;b\n1;2\n");
        assert_eq!(read_all(&f, &ReadOptions::default()).0.delimiter, b';');
    }

    #[test]
    fn shift_jis_files_are_detected_and_decoded() {
        let sjis = TextEncoding::ShiftJis
            .encode("名前,備考\n佐伯 光平,日本語のメモ\n")
            .unwrap();
        let f = file(&sjis);
        let (r, rows) = read_all(&f, &ReadOptions::default());
        assert_eq!(r.encoding, TextEncoding::ShiftJis);
        assert_eq!(r.headers(), ["名前", "備考"]);
        assert_eq!(rows[0], [s("佐伯 光平"), s("日本語のメモ")]);
    }

    #[test]
    fn a_large_shift_jis_file_decodes_across_chunk_boundaries() {
        let line = "日本語のテキスト,12345\n";
        let body = line.repeat(30_000); // 32 KiB の読み取り単位を何度もまたぐ
        let text = format!("a,b\n{body}");
        let sjis = TextEncoding::ShiftJis.encode(&text).unwrap();
        let f = file(&sjis);
        let (_, rows) = read_all(&f, &ReadOptions::default());
        assert_eq!(rows.len(), 30_000);
        assert!(
            rows.iter()
                .all(|r| r[0] == s("日本語のテキスト") && r[1] == s("12345"))
        );
    }

    #[test]
    fn a_ragged_row_is_reported() {
        let f = file(b"a,b\n1,2\n3\n");
        let mut r = CsvReader::open(f.path(), &ReadOptions::default()).unwrap();
        assert!(r.next_record().unwrap().is_some());
        assert!(matches!(r.next_record(), Err(IoError::Csv(_))));
    }

    #[test]
    fn an_empty_file_has_no_columns_and_preview_works() {
        let f = file(b"");
        let r = CsvReader::open(f.path(), &ReadOptions::default()).unwrap();
        assert!(r.headers().is_empty());
        let f = file(b"a,b\n1,2\n3,4\n5,6\n");
        let p = CsvReader::preview(f.path(), &ReadOptions::default(), 2).unwrap();
        assert_eq!((p.headers.len(), p.rows.len()), (2, 2));
    }
}
