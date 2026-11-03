//! 接続フォームで選べるデータベースの種類。`id` は core の `DriverRegistry` に登録する名前と同じ。

pub struct DriverKind {
    pub id: &'static str,
    pub label: &'static str,
    /// ネットワークで接続する DB の既定のポート。`None` は、ファイルを開く DB
    pub default_port: Option<u16>,
    /// 「データベース」欄の呼び名(Oracle ではサービス名)
    pub database_label: &'static str,
}

impl DriverKind {
    pub fn is_network(&self) -> bool {
        self.default_port.is_some()
    }
}

pub const KINDS: &[DriverKind] = &[
    DriverKind {
        id: "postgresql",
        label: "PostgreSQL",
        default_port: Some(5432),
        database_label: "データベース",
    },
    DriverKind {
        id: "mysql",
        label: "MySQL / MariaDB",
        default_port: Some(3306),
        database_label: "データベース",
    },
    DriverKind {
        id: "sqlserver",
        label: "SQL Server",
        default_port: Some(1433),
        database_label: "データベース",
    },
    DriverKind {
        id: "oracle",
        label: "Oracle",
        default_port: Some(1521),
        database_label: "サービス名",
    },
    DriverKind {
        id: "sqlite",
        label: "SQLite",
        default_port: None,
        database_label: "データベース",
    },
];

/// 名前から種類を探す。知らない名前(新しい版で保存された接続など)は、PostgreSQL として扱う。
pub fn find(id: &str) -> &'static DriverKind {
    KINDS.iter().find(|k| k.id == id).unwrap_or(&KINDS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_unique_id_and_unknown_ids_fall_back_to_the_first() {
        let mut ids: Vec<_> = KINDS.iter().map(|k| k.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KINDS.len());
        assert_eq!(find("mysql").default_port, Some(3306));
        assert_eq!(find("sqlserver").default_port, Some(1433));
        assert_eq!(find("oracle").database_label, "サービス名");
        assert_eq!(find("sqlite").default_port, None);
        assert_eq!(find("from-the-future").id, "postgresql");
    }
}
