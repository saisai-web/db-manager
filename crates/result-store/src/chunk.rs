use std::io;

use crate::StoreError;

const MAGIC: u32 = 0x5251_5343; // "CSQR"
const HEADER_LEN: usize = 4 + 8 + 4 + 4 + 8 + 4;

/// 連続したバイト列に詰めた行の塊。行ごとのオブジェクトは作らない。
///
/// セル `c`(行 × 列数 + 列)の値は `data[offsets[c]..offsets[c + 1]]`。
/// NULL のセルは長さ 0 で、`nulls` のビットが立つ。これにより NULL と空文字列を区別する。
#[derive(Debug)]
pub(crate) struct Chunk {
    pub first_row: u64,
    pub row_count: u32,
    ncols: u32,
    offsets: Vec<u32>,
    nulls: Vec<u64>,
    data: Vec<u8>,
}

impl Chunk {
    pub fn new(first_row: u64, ncols: usize) -> Self {
        Self {
            first_row,
            row_count: 0,
            ncols: ncols as u32,
            offsets: vec![0],
            nulls: Vec::new(),
            data: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    /// 行を追加する。セル数が合わない、または 4 GiB を超える場合は何も変えずに失敗する。
    pub fn push_row<'a>(
        &mut self,
        cells: impl IntoIterator<Item = Option<&'a [u8]>>,
    ) -> Result<(), StoreError> {
        let ncols = self.ncols as usize;
        let first_cell = self.row_count as usize * ncols;
        let data_mark = self.data.len();
        let offsets_mark = self.offsets.len();
        self.nulls.resize((first_cell + ncols).div_ceil(64), 0);

        let mut n = 0;
        let mut result = Ok(());
        for cell in cells {
            if n == ncols {
                n += 1;
                break;
            }
            match cell {
                Some(bytes) => {
                    if self.data.len() + bytes.len() > u32::MAX as usize {
                        result = Err(StoreError::RowTooLarge);
                        break;
                    }
                    self.data.extend_from_slice(bytes);
                }
                None => {
                    let bit = first_cell + n;
                    self.nulls[bit / 64] |= 1 << (bit % 64);
                }
            }
            self.offsets.push(self.data.len() as u32);
            n += 1;
        }
        if result.is_ok() && n != ncols {
            result = Err(StoreError::RowWidthMismatch {
                expected: ncols,
                actual: n,
            });
        }
        if result.is_err() {
            self.data.truncate(data_mark);
            self.offsets.truncate(offsets_mark);
            for bit in first_cell..first_cell + ncols {
                if bit / 64 < self.nulls.len() {
                    self.nulls[bit / 64] &= !(1 << (bit % 64));
                }
            }
            return result;
        }
        self.row_count += 1;
        Ok(())
    }

    /// `None` が NULL。行と列の範囲は呼び出し側が保証する。
    pub fn cell(&self, row: usize, col: usize) -> Option<&[u8]> {
        debug_assert!(row < self.row_count as usize && col < self.ncols as usize);
        let c = row * self.ncols as usize + col;
        if self.nulls[c / 64] >> (c % 64) & 1 == 1 {
            return None;
        }
        Some(&self.data[self.offsets[c] as usize..self.offsets[c + 1] as usize])
    }

    pub fn shrink(&mut self) {
        self.offsets.shrink_to_fit();
        self.nulls.shrink_to_fit();
        self.data.shrink_to_fit();
    }

    /// 保持しているバイト数の見積もり。
    pub fn size_bytes(&self) -> usize {
        self.offsets.capacity() * 4 + self.nulls.capacity() * 8 + self.data.capacity()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            HEADER_LEN + self.offsets.len() * 4 + self.nulls.len() * 8 + self.data.len(),
        );
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&self.first_row.to_le_bytes());
        out.extend_from_slice(&self.row_count.to_le_bytes());
        out.extend_from_slice(&self.ncols.to_le_bytes());
        out.extend_from_slice(&(self.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.nulls.len() as u32).to_le_bytes());
        for o in &self.offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        for w in &self.nulls {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend_from_slice(&self.data);
        out
    }

    /// 壊れたデータでも panic せず `InvalidData` を返す。
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        fn bad(msg: &str) -> io::Error {
            io::Error::new(io::ErrorKind::InvalidData, format!("spill chunk: {msg}"))
        }
        if buf.len() < HEADER_LEN {
            return Err(bad("short header"));
        }
        let u32_at = |i: usize| u32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
        let u64_at = |i: usize| u64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
        if u32_at(0) != MAGIC {
            return Err(bad("bad magic"));
        }
        let first_row = u64_at(4);
        let row_count = u32_at(12);
        let ncols = u32_at(16);
        let data_len = usize::try_from(u64_at(20)).map_err(|_| bad("data length"))?;
        let n_null_words = u32_at(28) as usize;

        let n_cells = (row_count as usize)
            .checked_mul(ncols as usize)
            .ok_or_else(|| bad("cell count"))?;
        let n_offsets = n_cells + 1;
        let expected = HEADER_LEN
            .checked_add(n_offsets.checked_mul(4).ok_or_else(|| bad("size"))?)
            .and_then(|v| v.checked_add(n_null_words.checked_mul(8)?))
            .and_then(|v| v.checked_add(data_len))
            .ok_or_else(|| bad("size"))?;
        if buf.len() != expected || n_null_words != n_cells.div_ceil(64) {
            return Err(bad("size mismatch"));
        }

        let mut p = HEADER_LEN;
        let offsets: Vec<u32> = buf[p..p + n_offsets * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        p += n_offsets * 4;
        let nulls: Vec<u64> = buf[p..p + n_null_words * 8]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| u64::from_le_bytes(*b))
            .collect();
        p += n_null_words * 8;
        let data = buf[p..].to_vec();

        if offsets[0] != 0
            || offsets.windows(2).any(|w| w[0] > w[1])
            || *offsets.last().unwrap() as usize != data.len()
        {
            return Err(bad("offsets"));
        }
        Ok(Self {
            first_row,
            row_count,
            ncols,
            offsets,
            nulls,
            data,
        })
    }
}

/// 表示範囲の 1 行への読み取り専用ビュー。
#[derive(Clone, Copy)]
pub struct RowView<'a> {
    pub(crate) chunk: &'a Chunk,
    pub(crate) row: usize,
}

impl<'a> RowView<'a> {
    pub fn col_count(&self) -> usize {
        self.chunk.ncols as usize
    }

    /// `None` が NULL。空文字列は `Some(&[])`。
    pub fn cell(&self, col: usize) -> Option<&'a [u8]> {
        assert!(col < self.col_count(), "column {col} out of range");
        self.chunk.cell(self.row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(cells: &'a [Option<&'a str>]) -> impl Iterator<Item = Option<&'a [u8]>> {
        cells.iter().map(|c| c.map(str::as_bytes))
    }

    #[test]
    fn null_and_empty_string_are_distinct() {
        let mut c = Chunk::new(0, 3);
        c.push_row(row(&[None, Some(""), Some("x")])).unwrap();
        assert_eq!(c.cell(0, 0), None);
        assert_eq!(c.cell(0, 1), Some(&b""[..]));
        assert_eq!(c.cell(0, 2), Some(&b"x"[..]));
    }

    #[test]
    fn width_mismatch_leaves_chunk_untouched() {
        let mut c = Chunk::new(0, 2);
        c.push_row(row(&[Some("a"), None])).unwrap();
        let before = (c.data_len(), c.offsets.len());
        assert!(matches!(
            c.push_row(row(&[Some("b")])),
            Err(StoreError::RowWidthMismatch {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            c.push_row(row(&[Some("b"), None, None])),
            Err(StoreError::RowWidthMismatch { .. })
        ));
        assert_eq!((c.data_len(), c.offsets.len()), before);
        c.push_row(row(&[None, Some("z")])).unwrap();
        assert_eq!(c.cell(0, 1), None);
        assert_eq!(c.cell(1, 0), None);
        assert_eq!(c.cell(1, 1), Some(&b"z"[..]));
    }

    #[test]
    fn rolled_back_null_bits_do_not_leak_into_next_row() {
        let mut c = Chunk::new(0, 2);
        assert!(c.push_row(row(&[None, None, None])).is_err());
        c.push_row(row(&[Some("a"), Some("b")])).unwrap();
        assert_eq!(c.cell(0, 0), Some(&b"a"[..]));
        assert_eq!(c.cell(0, 1), Some(&b"b"[..]));
    }

    #[test]
    fn binary_values_round_trip_through_encode() {
        let mut c = Chunk::new(42, 2);
        let bin: Vec<u8> = (0..=255).collect();
        for i in 0..100u8 {
            let bytes = [i, 0, 255];
            c.push_row([
                Some(&bin[..]),
                if i % 3 == 0 { None } else { Some(&bytes[..]) },
            ])
            .unwrap();
        }
        let d = Chunk::decode(&c.encode()).unwrap();
        assert_eq!(d.first_row, 42);
        assert_eq!(d.row_count, 100);
        for i in 0..100usize {
            assert_eq!(d.cell(i, 0), Some(&bin[..]));
            assert_eq!(d.cell(i, 1).is_none(), i % 3 == 0);
        }
    }

    #[test]
    fn decode_rejects_corrupt_input_without_panicking() {
        let mut c = Chunk::new(0, 2);
        c.push_row(row(&[Some("abc"), None])).unwrap();
        let good = c.encode();
        assert!(Chunk::decode(&good).is_ok());
        assert!(Chunk::decode(&[]).is_err());
        assert!(Chunk::decode(&good[..good.len() - 1]).is_err());
        let mut bad_magic = good.clone();
        bad_magic[0] ^= 0xff;
        assert!(Chunk::decode(&bad_magic).is_err());
        // offsets を壊す
        let mut bad_offsets = good.clone();
        bad_offsets[HEADER_LEN + 4] = 0xff;
        assert!(Chunk::decode(&bad_offsets).is_err());
        for i in 0..good.len() {
            let mut b = good.clone();
            b[i] = b[i].wrapping_add(1);
            let _ = Chunk::decode(&b); // panic しないこと
        }
    }
}
