//! 公開する型: エラー、設定、読み取り結果、統計。

use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// 退避が無効で、メモリ上限に達した。受信を止めること。
    #[error("結果のメモリ上限に達しました")]
    LimitReached,
    #[error("行のセル数が列数と一致しません(列数 {expected}、実際 {actual})")]
    RowWidthMismatch { expected: usize, actual: usize },
    #[error("1 チャンクに収まらない大きさの行です")]
    RowTooLarge,
    #[error("結果は受信済みで、行を追加できません")]
    Finished,
    #[error("全件を受信するまでソートできません")]
    NotComplete,
    #[error("列 {0} は範囲外です")]
    ColumnOutOfRange(usize),
    #[error("行数または値の大きさがソートできる上限を超えています")]
    TooLargeToSort,
    #[error("一時ファイルの操作に失敗しました: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// 結果をメモリに置く上限(バイト)。既定 512 MiB。
    pub memory_limit: usize,
    /// 退避先のディレクトリ。`None` なら退避せず、上限で受信を止める。
    pub spill_dir: Option<PathBuf>,
    /// 1 チャンクの最大行数
    pub chunk_rows: u32,
    /// 1 チャンクの最大バイト数。大きなセルが続いても 1 チャンクが肥大しない
    pub chunk_max_bytes: usize,
}

impl StoreConfig {
    pub const DEFAULT_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

    pub fn new(spill_dir: Option<PathBuf>) -> Self {
        Self {
            memory_limit: Self::DEFAULT_MEMORY_LIMIT,
            spill_dir,
            chunk_rows: 4096,
            chunk_max_bytes: 4 * 1024 * 1024,
        }
    }
}

/// 行の読み取り結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access<R> {
    Ready(R),
    /// 退避済みで読み戻し中。完了すると `set_on_ready` の通知が来る。
    Loading,
    /// 範囲外、または一時ファイルの読み戻しに失敗した。
    Unavailable,
}

/// 追記の勢い。`High` なら、退避の書き出しが追いついていない。
/// 受信側は少し待ってから続けること(行は失われず、メモリが膨らむのを防ぐための合図)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub rows: u64,
    /// メモリ上のチャンクの合計(受信中のチャンクを含む)
    pub resident_bytes: usize,
    /// 一時ファイルに書き出した合計
    pub spilled_bytes: u64,
    /// `resident_bytes` のうち、書き出し待ちで、完了すると解放される量
    pub pending_write_bytes: usize,
}
