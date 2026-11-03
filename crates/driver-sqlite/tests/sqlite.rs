//! SQLite ドライバの結合テスト(一時ファイルの DB)。

use std::time::{Duration, Instant};

use async_trait::async_trait;
use driver_api::{
    ConnectionConfig, Dialect, Driver, ErrorKind, Flow, KeyKind, ResultColumn, RowSink, Session,
    Statement, TableKind, TableRef, ValueClass,
};
use driver_sqlite::{SqliteDialect, SqliteDriver};

#[derive(Default)]
struct Collect {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Option<Vec<u8>>>>,
    pause_at: usize,
    resume_with: Vec<Flow>,
    resumed: usize,
    count: usize,
}

#[async_trait]
impl RowSink for Collect {
    fn columns(&mut self, c: &[ResultColumn]) -> Flow {
        self.columns = c.to_vec();
        Flow::Continue
    }
    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        self.count += 1;
        self.rows
            .push(cells.iter().map(|c| c.map(<[u8]>::to_vec)).collect());
        if self.pause_at != 0 && self.count.is_multiple_of(self.pause_at) {
            Flow::Pause
        } else {
            Flow::Continue
        }
    }
    async fn resume(&mut self) -> Flow {
        self.resumed += 1;
        if self.resume_with.is_empty() {
            Flow::Continue
        } else {
            self.resume_with.remove(0)
        }
    }
}

fn cfg(path: &std::path::Path) -> ConnectionConfig {
    ConnectionConfig::new(path.display().to_string())
}

async fn open(path: &std::path::Path) -> Box<dyn Session> {
    SqliteDriver.connect(&cfg(path)).await.expect("connect")
}

async fn run(s: &mut Box<dyn Session>, sql: &str) -> (driver_api::ExecSummary, Collect) {
    let mut sink = Collect::default();
    let sum = s
        .execute(&Statement::new(sql), &mut sink)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    (sum, sink)
}

fn text(c: &Collect, row: usize, col: usize) -> Option<String> {
    c.rows[row][col]
        .as_ref()
        .map(|b| String::from_utf8(b.clone()).unwrap())
}

#[tokio::test]
async fn values_come_back_exactly_with_null_distinct_from_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir.path().join("t.db")).await;
    assert!(s.server_info().version.starts_with("SQLite 3"));
    run(&mut s, "create table t (id integer primary key, price real, name text, bin blob, ok boolean, at datetime, extra)").await;
    run(&mut s, "insert into t values (9223372036854775807, 0.1, '日本語', x'00ff10', 1, '2026-11-02 12:34:56', null), (2, 3.0, '', null, 0, null, 'x')").await;
    let (sum, r) = run(&mut s, "select * from t order by id desc").await;
    assert_eq!((sum.rows_received, sum.affected_rows), (2, Some(2)));
    assert_eq!(
        text(&r, 0, 0).as_deref(),
        Some("9223372036854775807"),
        "a 64-bit integer is exact"
    );
    assert_eq!(text(&r, 0, 1).as_deref(), Some("0.1"));
    assert_eq!(text(&r, 0, 2).as_deref(), Some("日本語"));
    assert_eq!(
        r.rows[0][3].as_deref(),
        Some(&[0x00, 0xff, 0x10][..]),
        "a blob stays bytes"
    );
    assert_eq!(r.rows[1][2].as_deref(), Some(&b""[..]), "empty string");
    assert_eq!(r.rows[1][3], None, "NULL");
    assert_eq!(text(&r, 1, 1).as_deref(), Some("3.0"));
    let classes: Vec<_> = r.columns.iter().map(|c| c.class).collect();
    assert_eq!(
        classes,
        [
            ValueClass::Number,
            ValueClass::Number,
            ValueClass::Text,
            ValueClass::Binary,
            ValueClass::Bool,
            ValueClass::Temporal,
            ValueClass::Other
        ]
    );
}

#[tokio::test]
async fn dml_reports_affected_rows_and_ddl_none() {
    let mut s = SqliteDriver
        .connect(&ConnectionConfig::new(""))
        .await
        .unwrap(); // メモリ上の DB
    let (_, r) = run(&mut s, "create table t (a int)").await;
    assert!(r.columns.is_empty());
    assert_eq!(
        run(&mut s, "insert into t values (1), (2), (3)")
            .await
            .0
            .affected_rows,
        Some(3)
    );
    assert_eq!(
        run(&mut s, "update t set a = a + 1 where a > 1")
            .await
            .0
            .affected_rows,
        Some(2)
    );
    assert_eq!(
        run(&mut s, "delete from t where a = 99")
            .await
            .0
            .affected_rows,
        Some(0)
    );
}

#[tokio::test]
async fn syntax_errors_carry_a_character_position() {
    let mut s = SqliteDriver
        .connect(&ConnectionConfig::new(""))
        .await
        .unwrap();
    let sql = "SELECT '日本語', FROM t";
    let err = s
        .execute(&Statement::new(sql), &mut Collect::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    let pos = err.position.expect("position");
    let near: String = sql.chars().skip(pos).take(4).collect();
    assert_eq!(near, "FROM", "{err:?}");
    // エラーの後も使える
    run(&mut s, "select 1").await;
}

#[tokio::test]
async fn bound_parameters_are_values_and_null_safe_comparison_works() {
    let mut s = SqliteDriver
        .connect(&ConnectionConfig::new(""))
        .await
        .unwrap();
    run(
        &mut s,
        "create table t (id integer primary key, n integer, note text)",
    )
    .await;
    let d = SqliteDialect;
    let evil = "x'); DROP TABLE t; --";
    let insert = format!(
        "INSERT INTO t VALUES ({}, {}, {})",
        d.bind_text(1, "integer"),
        d.bind_text(2, "integer"),
        d.bind_text(3, "text")
    );
    let sum = s
        .execute(
            &Statement::with_params(
                insert,
                vec![
                    Some("9007199254740993".into()),
                    Some("42".into()),
                    Some(evil.into()),
                ],
            ),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));
    let (_, r) = run(&mut s, "select id, n, note, typeof(n) from t").await;
    assert_eq!(
        text(&r, 0, 0).as_deref(),
        Some("9007199254740993"),
        "beyond f64's exact integers"
    );
    assert_eq!(
        text(&r, 0, 3).as_deref(),
        Some("integer"),
        "text '42' is converted by the column's affinity"
    );
    assert_eq!(text(&r, 0, 2).as_deref(), Some(evil));

    // NULL どうしも一致させる比較で、元の値が NULL の行を更新できる
    run(&mut s, "insert into t (id) values (2)").await;
    let sql = format!(
        "UPDATE t SET note = {} WHERE id = {} AND {}",
        d.bind_text(1, "text"),
        d.bind_text(2, "integer"),
        d.null_safe_equals("note", &d.bind_text(3, "text"))
    );
    let sum = s
        .execute(
            &Statement::with_params(sql, vec![Some("filled".into()), Some("2".into()), None]),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    assert_eq!(sum.affected_rows, Some(1));
}

#[tokio::test]
async fn transactions_roll_back_and_commit_within_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir.path().join("t.db")).await;
    run(&mut s, "create table t (a int)").await;
    run(&mut s, "begin").await;
    run(&mut s, "insert into t values (1)").await;
    run(&mut s, "rollback").await;
    assert_eq!(
        text(&run(&mut s, "select count(*) from t").await.1, 0, 0).as_deref(),
        Some("0")
    );
    run(&mut s, "begin").await;
    run(&mut s, "insert into t values (1)").await;
    run(&mut s, "commit").await;
    assert_eq!(
        text(&run(&mut s, "select count(*) from t").await.1, 0, 0).as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn query_only_mode_rejects_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = open(&dir.path().join("t.db")).await;
    run(&mut s, "create table t (a int)").await;
    run(&mut s, &SqliteDialect.set_read_only_sql(true).unwrap()).await;
    let err = s
        .execute(
            &Statement::new("insert into t values (1)"),
            &mut Collect::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Query);
    run(&mut s, "select * from t").await;
    run(&mut s, &SqliteDialect.set_read_only_sql(false).unwrap()).await;
    run(&mut s, "insert into t values (1)").await;
}

const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 4000000000) SELECT count(*) FROM c";

#[tokio::test]
async fn cancel_interrupts_a_long_query_and_the_session_survives() {
    let mut s = SqliteDriver
        .connect(&ConnectionConfig::new(""))
        .await
        .unwrap();
    let canceller = s.canceller();
    let started = Instant::now();
    let exec = tokio::spawn(async move {
        let r = s
            .execute(&Statement::new(SLOW), &mut Collect::default())
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    canceller.cancel().await.unwrap();
    let (mut s, r) = exec.await.unwrap();
    assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        text(&run(&mut s, "select 'alive'").await.1, 0, 0).as_deref(),
        Some("alive")
    );
}

#[tokio::test]
async fn pause_resume_and_stop_free_the_connection() {
    let mut s = SqliteDriver
        .connect(&ConnectionConfig::new(""))
        .await
        .unwrap();
    let mut sink = Collect {
        pause_at: 1000,
        resume_with: vec![Flow::Continue, Flow::Stop],
        ..Collect::default()
    };
    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 50000000) SELECT x, 'v' || x FROM c";
    let started = Instant::now();
    let sum = s.execute(&Statement::new(sql), &mut sink).await.unwrap();
    assert!(sum.stopped_early);
    assert_eq!((sink.resumed, sink.count), (2, 2000));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        text(&run(&mut s, "select 'next'").await.1, 0, 0).as_deref(),
        Some("next")
    );
}

#[tokio::test]
async fn metadata_lists_tables_columns_keys_and_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.db");
    let mut s = open(&path).await;
    for sql in [
        "create table parent (id integer primary key, code text not null unique, name text default 'n/a')",
        "create table child (a int, b int, parent_id integer references parent(id), primary key (a, b))",
        "create index child_parent on child (parent_id)",
        "create unique index child_ab on child (b, a)",
        "create view v as select id, name from parent",
    ] {
        run(&mut s, sql).await;
    }
    let meta = SqliteDriver.connect_metadata(&cfg(&path)).await.unwrap();
    assert!(
        meta.schemas()
            .await
            .unwrap()
            .iter()
            .any(|x| x.name == "main")
    );
    let tables = meta.tables("main").await.unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("child", TableKind::Table),
            ("parent", TableKind::Table),
            ("v", TableKind::View)
        ]
    );

    let cols = meta
        .columns(&TableRef::new("main", "parent"))
        .await
        .unwrap();
    assert_eq!(
        cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["id", "code", "name"]
    );
    assert!(!cols[1].nullable);
    assert_eq!(cols[2].default.as_deref(), Some("'n/a'"));

    let keys = meta.keys(&TableRef::new("main", "child")).await.unwrap();
    let pk = keys.iter().find(|k| k.kind == KeyKind::Primary).unwrap();
    assert_eq!(pk.columns, ["a", "b"], "composite key in key order");
    let fk = keys.iter().find(|k| k.kind == KeyKind::Foreign).unwrap();
    assert_eq!(
        (fk.columns.as_slice(), fk.definition.as_deref()),
        (
            &["parent_id".to_string()][..],
            Some("REFERENCES parent(id)")
        )
    );
    let pkeys = meta.keys(&TableRef::new("main", "parent")).await.unwrap();
    assert!(
        pkeys
            .iter()
            .any(|k| k.kind == KeyKind::Unique && k.columns == ["code"])
    );

    let idx = meta.indexes(&TableRef::new("main", "child")).await.unwrap();
    let ab = idx.iter().find(|i| i.name == "child_ab").unwrap();
    assert!(ab.unique);
    assert_eq!(ab.columns, ["b", "a"]);
    assert!(ab.definition.contains("CREATE UNIQUE INDEX"));
    assert!(
        !idx.iter()
            .find(|i| i.name == "child_parent")
            .unwrap()
            .unique
    );
}

#[tokio::test]
async fn opening_an_unreachable_path_is_a_connect_error() {
    let err = SqliteDriver
        .connect(&ConnectionConfig::new("/nonexistent-dir/x/y.db"))
        .await
        .err()
        .expect("must fail");
    assert_eq!(err.kind, ErrorKind::Connect, "{err:?}");
}
