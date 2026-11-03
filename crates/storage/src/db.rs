use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    ConnectionOptions, ConnectionRecord, EnvLabel, HistoryEntry, HistoryQuery, HistoryStatus,
    NewConnection, NewHistory, Result, Snippet, StorageError,
};

/// スキーマの変更履歴。`PRAGMA user_version` が適用済みの数を表し、起動時に残りを順に適用する。
/// 既存の要素は変更せず、変更は末尾に追加する。
const MIGRATIONS: &[&str] = &[
    // 1: 初期スキーマ
    "
    CREATE TABLE connections (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL,
      driver      TEXT NOT NULL,
      host        TEXT,
      port        INTEGER,
      database    TEXT,
      username    TEXT,
      secret_ref  TEXT,
      options     TEXT NOT NULL,
      env_label   TEXT NOT NULL,
      created_at  TEXT NOT NULL,
      updated_at  TEXT NOT NULL
    );
    CREATE TABLE query_history (
      id             INTEGER PRIMARY KEY,
      connection_id  TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
      sql_text       TEXT NOT NULL,
      executed_at    TEXT NOT NULL,
      duration_ms    INTEGER,
      affected_rows  INTEGER,
      status         TEXT NOT NULL
    );
    CREATE INDEX query_history_by_connection ON query_history (connection_id, id DESC);
    CREATE TABLE snippets (
      id          TEXT PRIMARY KEY,
      name        TEXT NOT NULL,
      sql_text    TEXT NOT NULL,
      updated_at  TEXT NOT NULL
    );
    CREATE TABLE settings (
      key    TEXT PRIMARY KEY,
      value  TEXT NOT NULL
    );
    ",
];

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
const NEW_ID: &str = "lower(hex(randomblob(16)))";

/// アプリ内 SQLite。
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// ファイルを開く(なければ作る)。他のユーザーが読めないよう、権限を絞る。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let existed = path.exists();
        let conn = Connection::open(path)?;
        if !existed {
            restrict_file(path)?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&mut self) -> Result<()> {
        let supported = MIGRATIONS.len() as i64;
        let found = self.schema_version()?;
        if found > supported {
            return Err(StorageError::SchemaTooNew { found, supported });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            // 1 つの版は、全体が適用されるか、まったく適用されないかのどちらか
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    // ---- 接続設定 ----

    pub fn create_connection(&self, new: &NewConnection) -> Result<ConnectionRecord> {
        let id: String = self
            .conn
            .query_row(&format!("SELECT {NEW_ID}"), [], |r| r.get(0))?;
        self.conn.execute(
            &format!(
                "INSERT INTO connections
                   (id, name, driver, host, port, database, username, secret_ref, options,
                    env_label, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, {NOW}, {NOW})"
            ),
            params![
                id,
                new.name,
                new.driver,
                new.host,
                new.port,
                new.database,
                new.username,
                new.secret_ref,
                serde_json::to_string(&new.options)
                    .map_err(|e| StorageError::InvalidData(e.to_string()))?,
                new.env_label.as_str(),
            ],
        )?;
        self.connection(&id)?.ok_or(StorageError::NotFound(id))
    }

    pub fn connection(&self, id: &str) -> Result<Option<ConnectionRecord>> {
        self.conn
            .query_row(
                &format!("SELECT {CONNECTION_COLUMNS} FROM connections WHERE id = ?1"),
                [id],
                connection_row,
            )
            .optional()?
            .transpose()
    }

    /// 名前順
    pub fn connections(&self) -> Result<Vec<ConnectionRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CONNECTION_COLUMNS} FROM connections ORDER BY name COLLATE NOCASE, created_at"
        ))?;
        stmt.query_map([], connection_row)?.map(|r| r?).collect()
    }

    /// `id` 以外の項目を更新する。
    pub fn update_connection(&self, rec: &ConnectionRecord) -> Result<ConnectionRecord> {
        let n = self.conn.execute(
            &format!(
                "UPDATE connections SET name = ?2, driver = ?3, host = ?4, port = ?5, database = ?6,
                   username = ?7, secret_ref = ?8, options = ?9, env_label = ?10, updated_at = {NOW}
                 WHERE id = ?1"
            ),
            params![
                rec.id,
                rec.name,
                rec.driver,
                rec.host,
                rec.port,
                rec.database,
                rec.username,
                rec.secret_ref,
                serde_json::to_string(&rec.options)
                    .map_err(|e| StorageError::InvalidData(e.to_string()))?,
                rec.env_label.as_str(),
            ],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(rec.id.clone()));
        }
        self.connection(&rec.id)?
            .ok_or_else(|| StorageError::NotFound(rec.id.clone()))
    }

    /// 接続設定と、その実行履歴を削除する。資格情報ストア側の削除は呼び出し側が行う。
    pub fn delete_connection(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM connections WHERE id = ?1", [id])?
            > 0)
    }

    /// 接続設定を JSON に書き出す。パスワードも、その参照キーも含めない
    /// (別の PC では意味がなく、移行先で再入力するため)。
    pub fn export_connections(&self) -> Result<String> {
        let items: Vec<ExportedConnection> = self
            .connections()?
            .into_iter()
            .map(|c| ExportedConnection {
                name: c.name,
                driver: c.driver,
                host: c.host,
                port: c.port,
                database: c.database,
                username: c.username,
                options: c.options,
                env_label: c.env_label,
            })
            .collect();
        serde_json::to_string_pretty(&Export {
            version: 1,
            connections: items,
        })
        .map_err(|e| StorageError::InvalidData(e.to_string()))
    }

    /// [`export_connections`](Self::export_connections) の出力を取り込む。新しい id で登録し、
    /// 同名の接続があれば `名前 (2)` のように区別する。全件が登録されるか、1 件も登録されない。
    pub fn import_connections(&self, json: &str) -> Result<Vec<ConnectionRecord>> {
        let export: Export = serde_json::from_str(json)
            .map_err(|e| StorageError::InvalidData(format!("接続設定の形式が不正です: {e}")))?;
        if export.version != 1 {
            return Err(StorageError::InvalidData(format!(
                "未対応の版です: {}",
                export.version
            )));
        }
        let mut names: std::collections::HashSet<String> = self
            .connections()?
            .into_iter()
            .map(|c| c.name.to_lowercase())
            .collect();
        let tx = self.conn.unchecked_transaction()?;
        let mut created = Vec::new();
        for c in export.connections {
            let mut name = c.name.clone();
            let mut n = 1;
            while names.contains(&name.to_lowercase()) {
                n += 1;
                name = format!("{} ({n})", c.name);
            }
            names.insert(name.to_lowercase());
            let rec = self.create_connection(&NewConnection {
                name,
                driver: c.driver,
                host: c.host,
                port: c.port,
                database: c.database,
                username: c.username,
                secret_ref: None,
                options: c.options,
                env_label: c.env_label,
            })?;
            created.push(rec);
        }
        tx.commit()?;
        Ok(created)
    }

    // ---- 実行履歴 ----

    pub fn add_history(&self, h: &NewHistory) -> Result<i64> {
        self.conn.execute(
            &format!(
                "INSERT INTO query_history
                   (connection_id, sql_text, executed_at, duration_ms, affected_rows, status)
                 VALUES (?1, ?2, COALESCE(?3, {NOW}), ?4, ?5, ?6)"
            ),
            params![
                h.connection_id,
                h.sql_text,
                h.executed_at,
                h.duration_ms,
                h.affected_rows,
                h.status.as_str()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 新しい順
    pub fn history(&self, q: &HistoryQuery) -> Result<Vec<HistoryEntry>> {
        let pattern = q.contains.as_deref().map(|s| {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        });
        let limit = if q.limit == 0 { 100 } else { q.limit } as i64;
        let mut stmt = self.conn.prepare(
            "SELECT id, connection_id, sql_text, executed_at, duration_ms, affected_rows, status
             FROM query_history
             WHERE (?1 IS NULL OR connection_id = ?1)
               AND (?2 IS NULL OR sql_text LIKE ?2 ESCAPE '\\')
             ORDER BY id DESC LIMIT ?3",
        )?;
        stmt.query_map(params![q.connection_id, pattern, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .map(|row| {
            let (id, connection_id, sql_text, executed_at, duration_ms, affected_rows, status) =
                row?;
            Ok(HistoryEntry {
                id,
                connection_id,
                sql_text,
                executed_at,
                duration_ms,
                affected_rows,
                status: HistoryStatus::parse(&status)?,
            })
        })
        .collect()
    }

    pub fn clear_history(&self, connection_id: Option<&str>) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM query_history WHERE ?1 IS NULL OR connection_id = ?1",
            [connection_id],
        )?)
    }

    /// 接続ごとに、新しい `keep` 件だけを残す。
    pub fn prune_history(&self, connection_id: &str, keep: usize) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM query_history
             WHERE connection_id = ?1
               AND id NOT IN (SELECT id FROM query_history WHERE connection_id = ?1
                              ORDER BY id DESC LIMIT ?2)",
            params![connection_id, keep as i64],
        )?)
    }

    // ---- スニペット ----

    /// `id` が `None` なら新規、あれば更新する。
    pub fn save_snippet(&self, id: Option<&str>, name: &str, sql_text: &str) -> Result<Snippet> {
        let id = match id {
            Some(id) => id.to_string(),
            None => self
                .conn
                .query_row(&format!("SELECT {NEW_ID}"), [], |r| r.get(0))?,
        };
        self.conn.execute(
            &format!(
                "INSERT INTO snippets (id, name, sql_text, updated_at) VALUES (?1, ?2, ?3, {NOW})
                 ON CONFLICT(id) DO UPDATE SET name = ?2, sql_text = ?3, updated_at = {NOW}"
            ),
            params![id, name, sql_text],
        )?;
        Ok(self.conn.query_row(
            "SELECT id, name, sql_text, updated_at FROM snippets WHERE id = ?1",
            [&id],
            snippet_row,
        )?)
    }

    pub fn snippets(&self) -> Result<Vec<Snippet>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, sql_text, updated_at FROM snippets ORDER BY name COLLATE NOCASE",
        )?;
        Ok(stmt.query_map([], snippet_row)?.collect::<Result<_, _>>()?)
    }

    pub fn delete_snippet(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM snippets WHERE id = ?1", [id])?
            > 0)
    }

    // ---- 設定 ----

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }
}

const CONNECTION_COLUMNS: &str = "id, name, driver, host, port, database, username, secret_ref, \
                                  options, env_label, created_at, updated_at";

fn connection_row(r: &Row<'_>) -> rusqlite::Result<Result<ConnectionRecord>> {
    let options: String = r.get(8)?;
    let env: String = r.get(9)?;
    let port: Option<i64> = r.get(4)?;
    let rec = (|| -> Result<ConnectionRecord> {
        Ok(ConnectionRecord {
            id: r.get(0)?,
            name: r.get(1)?,
            driver: r.get(2)?,
            host: r.get(3)?,
            port: port
                .map(|p| {
                    u16::try_from(p).map_err(|_| StorageError::InvalidData(format!("port: {p}")))
                })
                .transpose()?,
            database: r.get(5)?,
            username: r.get(6)?,
            secret_ref: r.get(7)?,
            options: serde_json::from_str::<ConnectionOptions>(&options)
                .map_err(|e| StorageError::InvalidData(format!("options: {e}")))?,
            env_label: EnvLabel::parse(&env)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
        })
    })();
    Ok(rec)
}

fn snippet_row(r: &Row<'_>) -> rusqlite::Result<Snippet> {
    Ok(Snippet {
        id: r.get(0)?,
        name: r.get(1)?,
        sql_text: r.get(2)?,
        updated_at: r.get(3)?,
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Export {
    version: u32,
    connections: Vec<ExportedConnection>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ExportedConnection {
    name: String,
    driver: String,
    host: Option<String>,
    port: Option<u16>,
    database: Option<String>,
    username: Option<String>,
    options: ConnectionOptions,
    env_label: EnvLabel,
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}
