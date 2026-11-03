//! スキーマツリーの状態。取得は core がキャッシュするので、ここは表示用に写したものだけを持つ。

use std::collections::{HashMap, HashSet};

use sqltool_core::{SchemaInfo, TableDetails, TableInfo};

#[derive(Default)]
pub struct SchemaState {
    pub schemas: Option<Vec<SchemaInfo>>,
    pub tables: HashMap<String, Vec<TableInfo>>,
    pub details: HashMap<(String, String), TableDetails>,
    pub expanded: HashSet<String>,
    pub error: Option<String>,
}

/// ツリーのノードを区別するキー。
pub fn schema_key(schema: &str) -> String {
    format!("s:{schema}")
}

/// スキーマ名と表名の区切り。名前に現れない制御文字を使う(`.` は名前に含まれうる)。
pub const KEY_SEP: char = '\u{1f}';

pub fn table_key(schema: &str, table: &str) -> String {
    format!("t:{schema}{KEY_SEP}{table}")
}

impl SchemaState {
    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }

    /// 開いていれば閉じ、閉じていれば開く。開いた(true)なら、中身を読み込む必要がある。
    pub fn toggle(&mut self, key: &str) -> bool {
        if self.expanded.remove(key) {
            false
        } else {
            self.expanded.insert(key.to_string());
            true
        }
    }

    /// DDL の後など。取得し直すので、開閉の状態は残す。
    pub fn invalidate(&mut self) {
        self.schemas = None;
        self.tables.clear();
        self.details.clear();
        self.error = None;
    }
}
