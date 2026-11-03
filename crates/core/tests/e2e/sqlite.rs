//! 同じ core で SQLite を動かす。core が DB の名前で分岐していないことの確認でもある
//! (PostgreSQL のサーバーは要らないので、どの環境でも実行できる)。

use sqltool_core::*;

use crate::common::*;

struct Lite {
    h: H,
    tab: TabId,
    dir: tempfile::TempDir,
}

impl Lite {
    async fn start(env: EnvLabel) -> Self {
        let mut h = H::start_sqlite().await;
        let dir = tempfile::tempdir().unwrap();
        let tab = h.open_sqlite_tab(&dir.path().join("t.db"), env).await;
        Self { h, tab, dir }
    }

    async fn rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let evs = self.h.exec(self.tab, sql).await;
        rows(&result_handle(&evs))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_statements_and_see_results_just_like_with_postgresql() {
    let mut l = Lite::start(EnvLabel::Dev).await;
    let evs = l.h.exec(l.tab, "create table t (id integer primary key, name text); insert into t values (1, 'a'), (2, null), (3, ''); select * from t order by id").await;
    assert_eq!(status(&evs), ExecutionStatus::Completed);
    let r = result_handle(&evs);
    assert_eq!(
        rows(&r),
        [
            [cell("1"), cell("a")],
            [cell("2"), None],
            [cell("3"), cell("")]
        ]
    );
    assert_eq!(r.columns()[0].class, ValueClass::Number);
    // エラー位置も、エディタ上の位置に直る
    let sql = "select 1;\nselect '日本語', from t";
    let evs = l.h.exec(l.tab, sql).await;
    let StatementOutcome::Failed { editor_offset, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    let off = editor_offset.expect("offset");
    assert_eq!(&sql[off..off + 4], "from");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safety_rules_work_with_sqlites_dialect() {
    let mut l = Lite::start(EnvLabel::Production).await;
    // 本番ラベルは読み取り専用で始まり、書き込みは、1 文も実行されない
    let evs = l.h.exec(l.tab, "select 1; create table t (a int)").await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    l.h.core.send(Command::SetReadOnly {
        tab: l.tab,
        read_only: false,
    });
    l.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    l.h.exec(
        l.tab,
        "create table t (a int); insert into t values (1), (2)",
    )
    .await;

    // DB 側の読み取り専用(PRAGMA query_only)でも止まる
    l.h.core.send(Command::SetReadOnly {
        tab: l.tab,
        read_only: true,
    });
    l.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: true,
                ..
            }
        )
    })
    .await;
    l.h.core.send(Command::SetReadOnly {
        tab: l.tab,
        read_only: false,
    });
    l.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;

    // WHERE のない DELETE は、確認が出る。断ると、実行されない
    l.h.send_exec(
        l.tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs =
        l.h.until(|e| matches!(e, Event::ConfirmationRequired { .. }))
            .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items[0].kind, ConfirmKind::UnfilteredDelete);
    l.h.core.send(Command::Confirm { id, accept: false });
    l.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(l.rows("select count(*) from t").await[0][0], cell("2"));

    // トランザクションの状態も追跡できる
    let evs = l.h.exec(l.tab, "begin").await;
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::TransactionChanged {
            state: TxnState::Open,
            ..
        }
    )));
    l.h.exec(l.tab, "rollback").await;
    assert!(l.h.core.pending_transactions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn table_editing_with_optimistic_conflict_detection() {
    let mut l = Lite::start(EnvLabel::Dev).await;
    l.h.exec(l.tab, "create table people (id integer primary key, name text, note text); insert into people values (1, 'Alice', null), (2, 'Bob', 'x')").await;
    l.h.core.send(Command::OpenTable {
        tab: l.tab,
        table: TableRef::new("main", "people"),
    });
    let evs =
        l.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
            .await;
    let r = result_handle(&evs);
    let EditAccess::Editable(source) = r.edit_access() else {
        panic!("{:?}", r.edit_access())
    };
    assert_eq!(source.key, ["id"]);

    let orig = |row: u64, col: usize| {
        r.read_row_blocking(row).unwrap().unwrap()[col]
            .clone()
            .map(|b| String::from_utf8(b).unwrap())
    };
    let names: Vec<String> = r.columns().iter().map(|c| c.name.clone()).collect();
    let prepare = |l: &mut Lite, b: &EditBuffer| {
        l.h.core.send(Command::PrepareEdit {
            tab: l.tab,
            result: r.id(),
            changes: b.to_changes(source, &names, orig),
        });
    };

    // 元が NULL のセルを変えて、削除と追加も 1 回で保存する
    let mut b = EditBuffer::new();
    b.set_cell(0, 2, Some("filled".into()), &orig(0, 2));
    b.delete_row(1);
    let ix = b.add_insert(3);
    b.set_insert_cell(ix, 1, Some(Some("Carol".into())));
    prepare(&mut l, &b);
    let evs =
        l.h.until(|e| matches!(e, Event::EditPreview { .. } | Event::CommandFailed { .. }))
            .await;
    let Event::EditPreview {
        plan, statements, ..
    } = evs.last().unwrap().clone()
    else {
        panic!("{evs:?}")
    };
    assert!(
        statements
            .iter()
            .any(|s| s.sql.contains("\"note\" IS NULL")),
        "{statements:?}"
    );
    l.h.core.send(Command::CommitEdit { tab: l.tab, plan });
    let done =
        l.h.until(|e| matches!(e, Event::EditCommitted { .. } | Event::EditRejected { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(done, Event::EditCommitted { statements: 3, .. }),
        "{done:?}"
    );
    let got = l
        .rows("select id, name, note from people order by id")
        .await;
    assert_eq!(
        got,
        [
            [cell("1"), cell("Alice"), cell("filled")],
            // id を指定しない追加は、SQLite が採番する(削除した行の id は空いているので、2 になる)
            [cell("2"), cell("Carol"), None]
        ]
    );

    // 他の接続が先に更新した行は、上書きしない(全体を取り消す)
    let mut b = EditBuffer::new();
    b.set_cell(0, 1, Some("mine".into()), &orig(0, 1));
    prepare(&mut l, &b);
    let evs = l.h.until(|e| matches!(e, Event::EditPreview { .. })).await;
    let Event::EditPreview { plan, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    l.h.exec(l.tab, "update people set name = 'theirs' where id = 1")
        .await;
    l.h.core.send(Command::CommitEdit { tab: l.tab, plan });
    let done =
        l.h.until(|e| matches!(e, Event::EditCommitted { .. } | Event::EditRejected { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(
            done,
            Event::EditRejected {
                reason: EditRejection::RowCount { actual: 0, .. },
                ..
            }
        ),
        "{done:?}"
    );
    assert_eq!(
        l.rows("select name from people where id = 1").await[0][0],
        cell("theirs")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_fetch_limit_sorting_and_schema_browsing() {
    let mut l = Lite::start(EnvLabel::Dev).await;
    // 取得上限で止まり、続きの取得と並べ替え(全件を受信した後)ができる
    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 3000) SELECT x, x * 1.5 AS y FROM c";
    l.h.send_exec(
        l.tab,
        sql,
        ExecuteTarget::All,
        ExecuteOptions {
            fetch_limit: FetchLimit::Rows(1000),
            ..Default::default()
        },
    );
    let evs = l.h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let Event::FetchPaused { result, rows: 1000 } = evs.last().unwrap().clone() else {
        panic!("{evs:?}")
    };
    let handle = result_handle(&evs);
    l.h.core.send(Command::FetchMore {
        result,
        more: FetchMore::Rest,
    });
    l.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(handle.row_count(), 3000);
    l.h.core.send(Command::SortResult {
        result,
        column: 0,
        order: SortOrder::Descending,
    });
    l.h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.with_row(0, |r| r.cell(0).map(<[u8]>::to_vec)),
        Access::Ready(Some(b"3000".to_vec()))
    );

    // 実行中のクエリを止められる
    let slow = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 4000000000) SELECT count(*) FROM c";
    l.h.send_exec(l.tab, slow, ExecuteTarget::All, ExecuteOptions::default());
    l.h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    l.h.core.send(Command::Cancel { tab: l.tab });
    let evs =
        l.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
            .await;
    assert_eq!(status(&evs), ExecutionStatus::Cancelled);

    // スキーマツリー
    let created = l.h.exec(l.tab, "create table parent (id integer primary key); create table child (id integer primary key, pid integer references parent(id))").await;
    let conn = l.h.core_connection_id(l.tab);
    l.h.core.send(Command::LoadTables {
        connection_id: conn.clone(),
        schema: "main".into(),
    });
    let evs = l.h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["child", "parent"],
        "created: {created:?}\nloaded: {evs:?}"
    );
    l.h.core.send(Command::LoadTableDetails {
        connection_id: conn,
        table: TableRef::new("main", "child"),
    });
    let evs = l.h.until(|e| matches!(e, Event::TableDetails { .. })).await;
    let Event::TableDetails { details, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(details.keys.iter().any(|k| k.kind == KeyKind::Foreign));
    let _ = &l.dir;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn csv_import_and_export_work_on_sqlite() {
    let mut l = Lite::start(EnvLabel::Dev).await;
    l.h.exec(
        l.tab,
        "create table items (id integer primary key, name text, price real)",
    )
    .await;
    let csv = l.dir.path().join("in.csv");
    std::fs::write(&csv, "id,name,price\n1,商品A,1.5\n2,,2.25\n").unwrap();
    l.h.core.send(Command::ImportCsv {
        tab: l.tab,
        table: TableRef::new("main", "items"),
        path: csv,
        options: CsvReadOptions::default(),
        mapping: vec![(0, "id".into()), (1, "name".into()), (2, "price".into())],
    });
    let done =
        l.h.until(|e| matches!(e, Event::ImportFinished { .. } | Event::ImportFailed { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(done, Event::ImportFinished { rows: 2, .. }),
        "{done:?}"
    );

    let evs = l.h.exec(l.tab, "select * from items order by id").await;
    let r = result_handle(&evs);
    let out = l.dir.path().join("out.csv");
    l.h.core.send(Command::ExportResult {
        result: r.id(),
        path: out.clone(),
        format: ExportFormat::Csv(CsvOptions {
            line_ending: "\n",
            ..Default::default()
        }),
    });
    l.h.until(|e| matches!(e, Event::ExportFinished { .. }))
        .await;
    assert_eq!(
        std::fs::read_to_string(out).unwrap(),
        "id,name,price\n1,商品A,1.5\n2,,2.25\n"
    );
}
