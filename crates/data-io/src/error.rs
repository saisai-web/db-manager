use crate::encoding::TextEncoding;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error(
        "{encoding:?} で表せない文字「{ch}」があります({row} 行目、列「{column}」)。UTF-8 で書き出してください"
    )]
    Unmappable {
        ch: char,
        row: u64,
        column: String,
        encoding: TextEncoding,
    },
    #[error("ファイルの操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV の形式が不正です: {0}")]
    Csv(String),
    #[error("Excel ファイルの書き出しに失敗しました: {0}")]
    Xlsx(String),
    #[error("{row} 行目、列「{column}」の値が Excel の 1 セルの上限(32,767 文字)を超えています")]
    CellTooLong { row: u64, column: String },
    #[error("書き出しを中止しました")]
    Cancelled,
}
