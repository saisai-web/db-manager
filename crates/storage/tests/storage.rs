use storage::{
    ConnectionOptions, EnvLabel, HistoryQuery, HistoryStatus, MemorySecretStore, NewConnection,
    NewHistory, SecretStore, Storage, StorageError, TlsSetting,
};

fn new_conn(name: &str) -> NewConnection {
    NewConnection {
        name: name.into(),
        driver: "postgresql".into(),
        host: Some("localhost".into()),
        port: Some(5432),
        database: Some("app".into()),
        username: Some("alice".into()),
        secret_ref: None,
        options: ConnectionOptions::default(),
        env_label: EnvLabel::Dev,
    }
}

fn history(conn: &str, sql: &str) -> NewHistory {
    NewHistory {
        connection_id: conn.into(),
        sql_text: sql.into(),
        duration_ms: Some(12),
        affected_rows: Some(3),
        status: HistoryStatus::Ok,
        executed_at: None,
    }
}

#[test]
fn fresh_database_is_migrated_and_reopening_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/dir/app.db");
    {
        let s = Storage::open(&path).unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
        s.create_connection(&new_conn("a")).unwrap();
    }
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 1);
    assert_eq!(s.connections().unwrap().len(), 1, "data survives reopening");
}

#[test]
fn refuses_a_database_from_a_newer_version_without_touching_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", 99).unwrap();
        c.execute_batch("CREATE TABLE marker (x)").unwrap();
    }
    match Storage::open(&path) {
        Err(StorageError::SchemaTooNew {
            found: 99,
            supported,
        }) => assert!(supported < 99),
        other => panic!("unexpected {:?}", other.err()),
    }
    let c = rusqlite::Connection::open(&path).unwrap();
    let v: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 99);
}

#[cfg(unix)]
#[test]
fn data_directory_and_file_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("SQLTool");
    let path = data.join("app.db");
    Storage::open(&path).unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn connection_crud_round_trips_every_field() {
    let s = Storage::open_in_memory().unwrap();
    let mut new = new_conn("Production DB");
    new.env_label = EnvLabel::Production;
    new.secret_ref = Some("connection:abc".into());
    new.options.tls = TlsSetting::SkipVerify;
    new.options.connect_timeout_secs = 30;
    let rec = s.create_connection(&new).unwrap();
    assert_eq!(rec.id.len(), 32);
    assert!(
        rec.created_at.ends_with('Z') && rec.created_at.contains('T'),
        "{}",
        rec.created_at
    );

    let got = s.connection(&rec.id).unwrap().unwrap();
    assert_eq!(got, rec);
    assert_eq!(got.env_label, EnvLabel::Production);
    assert_eq!(got.options.tls, TlsSetting::SkipVerify);
    assert_eq!(got.secret_ref.as_deref(), Some("connection:abc"));

    let mut edited = got.clone();
    edited.name = "Renamed".into();
    edited.port = None;
    edited.host = None;
    let updated = s.update_connection(&edited).unwrap();
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.port, None);
    assert_eq!(updated.created_at, rec.created_at);
    assert!(updated.updated_at >= rec.updated_at);

    assert!(s.delete_connection(&rec.id).unwrap());
    assert!(!s.delete_connection(&rec.id).unwrap());
    assert!(s.connection(&rec.id).unwrap().is_none());
    edited.id = "missing".into();
    assert!(matches!(
        s.update_connection(&edited),
        Err(StorageError::NotFound(_))
    ));
}

#[test]
fn connections_are_listed_by_name_ignoring_case() {
    let s = Storage::open_in_memory().unwrap();
    for n in ["beta", "Alpha", "gamma"] {
        s.create_connection(&new_conn(n)).unwrap();
    }
    let names: Vec<_> = s
        .connections()
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["Alpha", "beta", "gamma"]);
}

#[test]
fn unknown_option_keys_written_by_a_newer_version_are_preserved() {
    let s = Storage::open_in_memory().unwrap();
    let rec = s.create_connection(&new_conn("a")).unwrap();
    // 新しい版が書いたと想定して、未知のキーを持つ JSON を直接入れる
    let mut opts = rec.options.clone();
    opts.extra.insert(
        "ssh".into(),
        serde_json::json!({"host": "bastion", "port": 22}),
    );
    let mut edited = rec.clone();
    edited.options = opts;
    s.update_connection(&edited).unwrap();
    let back = s.connection(&rec.id).unwrap().unwrap();
    assert_eq!(back.options.extra["ssh"]["host"], "bastion");
    // 更新しても消えない
    let again = s.update_connection(&back).unwrap();
    assert_eq!(again.options.extra["ssh"]["port"], 22);
}

#[test]
fn history_is_newest_first_filtered_and_scoped_to_a_connection() {
    let s = Storage::open_in_memory().unwrap();
    let a = s.create_connection(&new_conn("a")).unwrap();
    let b = s.create_connection(&new_conn("b")).unwrap();
    s.add_history(&history(&a.id, "SELECT 1")).unwrap();
    s.add_history(&history(&a.id, "select * from Users"))
        .unwrap();
    s.add_history(&history(&b.id, "SELECT 100%")).unwrap();
    let mut failed = history(&a.id, "SELEC oops");
    failed.status = HistoryStatus::Error;
    failed.affected_rows = None;
    s.add_history(&failed).unwrap();

    let all_a = s
        .history(&HistoryQuery {
            connection_id: Some(a.id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        all_a
            .iter()
            .map(|h| h.sql_text.as_str())
            .collect::<Vec<_>>(),
        ["SELEC oops", "select * from Users", "SELECT 1"]
    );
    assert_eq!(all_a[0].status, HistoryStatus::Error);
    assert_eq!(all_a[0].affected_rows, None);
    assert_eq!(all_a[1].duration_ms, Some(12));

    let search = |text: &str| {
        s.history(&HistoryQuery {
            contains: Some(text.into()),
            limit: 10,
            ..Default::default()
        })
        .unwrap()
        .len()
    };
    assert_eq!(search("users"), 1, "case-insensitive for ASCII");
    assert_eq!(search("100%"), 1, "% is literal");
    assert_eq!(search("%"), 1, "% must not act as a wildcard");
    assert_eq!(search("_"), 0, "_ must not act as a wildcard");
    assert_eq!(
        search("SELECT"),
        3,
        "\"SELEC oops\" does not contain SELECT"
    );
    assert_eq!(search("SELEC"), 4);

    let limited = s
        .history(&HistoryQuery {
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(limited.len(), 2);
}

#[test]
fn history_keeps_multibyte_sql_and_explicit_timestamps() {
    let s = Storage::open_in_memory().unwrap();
    let c = s.create_connection(&new_conn("a")).unwrap();
    let mut h = history(&c.id, "SELECT '日本語'");
    h.executed_at = Some("2026-11-02T03:04:05.678Z".into());
    s.add_history(&h).unwrap();
    let got = &s.history(&HistoryQuery::default()).unwrap()[0];
    assert_eq!(got.sql_text, "SELECT '日本語'");
    assert_eq!(got.executed_at, "2026-11-02T03:04:05.678Z");
}

#[test]
fn deleting_a_connection_deletes_its_history() {
    let s = Storage::open_in_memory().unwrap();
    let a = s.create_connection(&new_conn("a")).unwrap();
    let b = s.create_connection(&new_conn("b")).unwrap();
    s.add_history(&history(&a.id, "SELECT 1")).unwrap();
    s.add_history(&history(&b.id, "SELECT 2")).unwrap();
    s.delete_connection(&a.id).unwrap();
    let left = s.history(&HistoryQuery::default()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].connection_id, b.id);
}

#[test]
fn history_for_an_unknown_connection_is_rejected() {
    let s = Storage::open_in_memory().unwrap();
    assert!(s.add_history(&history("nope", "SELECT 1")).is_err());
}

#[test]
fn prune_and_clear_history() {
    let s = Storage::open_in_memory().unwrap();
    let a = s.create_connection(&new_conn("a")).unwrap();
    let b = s.create_connection(&new_conn("b")).unwrap();
    for i in 0..10 {
        s.add_history(&history(&a.id, &format!("q{i}"))).unwrap();
    }
    s.add_history(&history(&b.id, "other")).unwrap();
    assert_eq!(s.prune_history(&a.id, 3).unwrap(), 7);
    let kept: Vec<_> = s
        .history(&HistoryQuery {
            connection_id: Some(a.id.clone()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|h| h.sql_text)
        .collect();
    assert_eq!(kept, ["q9", "q8", "q7"]);
    assert_eq!(
        s.history(&HistoryQuery {
            connection_id: Some(b.id.clone()),
            ..Default::default()
        })
        .unwrap()
        .len(),
        1
    );

    assert_eq!(s.clear_history(Some(&a.id)).unwrap(), 3);
    assert_eq!(s.clear_history(None).unwrap(), 1);
}

#[test]
fn snippets_settings_round_trip() {
    let s = Storage::open_in_memory().unwrap();
    let sn = s
        .save_snippet(None, "top users", "SELECT * FROM users LIMIT 10")
        .unwrap();
    let again = s
        .save_snippet(Some(&sn.id), "Top users", "SELECT 1")
        .unwrap();
    assert_eq!(again.id, sn.id);
    assert_eq!(again.sql_text, "SELECT 1");
    s.save_snippet(None, "alpha", "x").unwrap();
    let names: Vec<_> = s.snippets().unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["alpha", "Top users"]);
    assert!(s.delete_snippet(&sn.id).unwrap());
    assert_eq!(s.snippets().unwrap().len(), 1);

    assert_eq!(s.setting("memory_limit_mb").unwrap(), None);
    s.set_setting("memory_limit_mb", "512").unwrap();
    s.set_setting("memory_limit_mb", "256").unwrap();
    assert_eq!(
        s.setting("memory_limit_mb").unwrap().as_deref(),
        Some("256")
    );
    s.delete_setting("memory_limit_mb").unwrap();
    assert_eq!(s.setting("memory_limit_mb").unwrap(), None);
}

#[test]
fn export_contains_neither_passwords_nor_secret_refs_and_import_renames_duplicates() {
    let s = Storage::open_in_memory().unwrap();
    let mut prod = new_conn("prod");
    prod.env_label = EnvLabel::Production;
    prod.secret_ref = Some("connection:SECRET-REF-123".into());
    s.create_connection(&prod).unwrap();
    s.create_connection(&new_conn("dev")).unwrap();

    let json = s.export_connections().unwrap();
    assert!(!json.contains("SECRET-REF-123"));
    assert!(!json.contains("secret_ref"));
    assert!(!json.contains("password"));

    // 同じ DB に取り込むと、名前が重なるので区別される
    let imported = s.import_connections(&json).unwrap();
    let mut names: Vec<_> = imported.iter().map(|c| c.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["dev (2)", "prod (2)"]);
    assert!(
        imported.iter().all(|c| c.secret_ref.is_none()),
        "password must be re-entered"
    );
    let prod2 = imported.iter().find(|c| c.name == "prod (2)").unwrap();
    assert_eq!(
        prod2.env_label,
        EnvLabel::Production,
        "production label travels with the config"
    );
    // 2 回目も衝突しない
    let third = s.import_connections(&json).unwrap();
    assert!(third.iter().any(|c| c.name == "prod (3)"));
}

#[test]
fn import_is_all_or_nothing() {
    let s = Storage::open_in_memory().unwrap();
    assert!(s.import_connections("not json").is_err());
    assert!(
        s.import_connections(r#"{"version": 2, "connections": []}"#)
            .is_err()
    );
    let bad_label = r#"{"version":1,"connections":[
        {"name":"ok","driver":"postgresql","host":null,"port":null,"database":null,"username":null,
         "options":{},"env_label":"dev"},
        {"name":"bad","driver":"postgresql","host":null,"port":null,"database":null,"username":null,
         "options":{},"env_label":"nonsense"}]}"#;
    assert!(s.import_connections(bad_label).is_err());
    assert!(
        s.connections().unwrap().is_empty(),
        "nothing is imported on failure"
    );
}

#[test]
fn the_database_file_never_contains_the_password() {
    // 保存の手順(core が行う): パスワードは SecretStore、SQLite には参照キーだけ
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    let secrets = MemorySecretStore::new();
    let password = "correct-horse-battery-staple-7731";
    {
        let s = Storage::open(&path).unwrap();
        let mut new = new_conn("db");
        new.secret_ref = Some("connection:pending".into());
        let rec = s.create_connection(&new).unwrap();
        let key = format!("connection:{}", rec.id);
        secrets.set(&key, password).unwrap();
        let mut rec = rec;
        rec.secret_ref = Some(key.clone());
        s.update_connection(&rec).unwrap();
        s.add_history(&history(&rec.id, "SELECT 1")).unwrap();
        assert_eq!(secrets.get(&key).unwrap().as_deref(), Some(password));
    }
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(password.len())
                .any(|w| w == password.as_bytes()),
            "password found on disk"
        );
    }
}
