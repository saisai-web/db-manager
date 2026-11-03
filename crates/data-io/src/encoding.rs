//! 文字コード。既定は UTF-8、Windows では UTF-8(BOM 付き)や Shift_JIS も選べる(設計書の OS 差分)。

use std::borrow::Cow;

use encoding_rs::{SHIFT_JIS, UTF_8};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    #[default]
    Utf8,
    /// Excel が文字コードを正しく判定できるよう、先頭に BOM を付ける
    Utf8Bom,
    ShiftJis,
}

pub const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

impl TextEncoding {
    /// 文字列を符号化する。表せない文字があれば、その文字を返す(代替文字には置き換えない)。
    pub fn encode<'a>(self, s: &'a str) -> Result<Cow<'a, [u8]>, char> {
        match self {
            Self::Utf8 | Self::Utf8Bom => Ok(Cow::Borrowed(s.as_bytes())),
            Self::ShiftJis => {
                let (bytes, _, had_errors) = SHIFT_JIS.encode(s);
                if !had_errors {
                    return Ok(bytes);
                }
                // 表せなかった最初の文字を探す
                let bad = s
                    .chars()
                    .find(|c| {
                        let mut buf = [0u8; 4];
                        SHIFT_JIS.encode(c.encode_utf8(&mut buf)).2
                    })
                    .unwrap_or('?');
                Err(bad)
            }
        }
    }

    pub fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8Bom => UTF8_BOM,
            _ => &[],
        }
    }
}

/// ファイルの先頭から、文字コードを推定する。BOM があれば UTF-8(BOM 付き)、
/// UTF-8 として正しければ UTF-8、そうでなければ Shift_JIS とみなす。
pub fn detect(head: &[u8]) -> TextEncoding {
    if head.starts_with(UTF8_BOM) {
        return TextEncoding::Utf8Bom;
    }
    // 先頭の一部だけを見るので、末尾で切れた多バイト文字は、正しいものとして扱う
    match std::str::from_utf8(head) {
        Ok(_) => TextEncoding::Utf8,
        Err(e) if e.error_len().is_none() => TextEncoding::Utf8,
        Err(_) => TextEncoding::ShiftJis,
    }
}

/// バイト列を文字列にする(BOM は除く)。不正な並びは置換文字になる。
pub fn decode(bytes: &[u8], enc: TextEncoding) -> String {
    match enc {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
            UTF_8.decode_with_bom_removal(bytes).0.into_owned()
        }
        TextEncoding::ShiftJis => SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_jis_encodes_japanese_and_rejects_unmappable_characters() {
        let ok = TextEncoding::ShiftJis.encode("日本語ABC").unwrap();
        assert_eq!(decode(&ok, TextEncoding::ShiftJis), "日本語ABC");
        assert_eq!(TextEncoding::ShiftJis.encode("emoji 😀 here"), Err('😀'));
        assert!(TextEncoding::Utf8.encode("😀").is_ok());
    }

    #[test]
    fn detection_prefers_bom_then_utf8_then_shift_jis() {
        assert_eq!(detect(b"\xEF\xBB\xBFabc"), TextEncoding::Utf8Bom);
        assert_eq!(detect("日本語".as_bytes()), TextEncoding::Utf8);
        let sjis = TextEncoding::ShiftJis.encode("日本語").unwrap();
        assert_eq!(detect(&sjis), TextEncoding::ShiftJis);
        // 多バイト文字の途中で切れた UTF-8 は、UTF-8 のまま
        let cut = &"日本語".as_bytes()[..4];
        assert_eq!(detect(cut), TextEncoding::Utf8);
        assert_eq!(detect(b""), TextEncoding::Utf8);
    }

    #[test]
    fn decoding_removes_the_bom() {
        assert_eq!(decode(b"\xEF\xBB\xBFhi", TextEncoding::Utf8Bom), "hi");
    }
}
