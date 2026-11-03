//! 保存する変更。

/// セルの値。`None` が NULL(空文字列とは別)。値は DB が返した文字列のまま扱う。
pub type Value = Option<String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Update,
    Insert,
    Delete,
}

/// 1 行ぶんの変更。列は名前で指す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Update {
        /// 更新する行を特定するキー(元の値)
        key: Vec<(String, Value)>,
        /// 変更した列: (列名, 新しい値, 元の値)。元の値は、他の人が先に更新した行を
        /// 上書きしないために、WHERE に含める
        set: Vec<(String, Value, Value)>,
    },
    Insert {
        /// 値を指定した列。指定しない列は、DB の既定値になる
        values: Vec<(String, Value)>,
    },
    Delete {
        key: Vec<(String, Value)>,
    },
}

impl Change {
    pub fn kind(&self) -> ChangeKind {
        match self {
            Self::Update { .. } => ChangeKind::Update,
            Self::Insert { .. } => ChangeKind::Insert,
            Self::Delete { .. } => ChangeKind::Delete,
        }
    }
}
