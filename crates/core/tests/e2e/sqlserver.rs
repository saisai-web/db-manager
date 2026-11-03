//! 同じ core で SQL Server を動かす。実行、安全確認、編集、取得上限、スキーマ、取り込みと書き出し。
//! サーバーを用意できなければ飛ばす(`SQLTOOL_REQUIRE_MSSQL=1` なら失敗させる)。

use sqltool_core::*;

use crate::common::*;

macro_rules! ms {
    () => {
        match test_sqlserver::database_or_skip() {
            Some(db) => db,
            None => return,
        }
    };
}

mod plan;

struct Ms {
    h: H,
    tab: TabId,
    dir: tempfile::TempDir,
}

impl Ms {
    async fn start(db: test_sqlserver::TestDb, env: EnvLabel) -> Self {
        let mut h = H::start_without_pg().await;
        let tab = h
            .open_net_tab(
                "sqlserver",
                NetConn {
                    host: &db.host,
                    port: db.port,
                    user: &db.user,
                    password: db.password.as_deref(),
                    database: &db.dbname,
                },
                env,
            )
            .await;
        Self {
            h,
            tab,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn table(&self, name: &str) -> TableRef {
        TableRef::new("dbo", name)
    }

    async fn rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let evs = self.h.exec(self.tab, sql).await;
        rows(&result_handle(&evs))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_statements_and_see_results_just_like_with_postgresql() {
    let mut m = Ms::start(ms!(), EnvLabel::Dev).await;
    let evs = m.h.exec(m.tab, "create table t (id int primary key, name nvarchar(50)); insert into t values (1, 'a'), (2, null), (3, ''); select * from t order by id").await;
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

    // 日本語は、そのまま往復する
    let got = m.rows("select N'日本語', N'a\\b'").await;
    assert_eq!(got[0], [cell("日本語"), cell("a\\b")]);

    // エラー位置も、エディタ上の位置に直る(2 文目の中の位置)
    let sql = "select 1;\nselect N'日本語' frm tbl";
    let evs = m.h.exec(m.tab, sql).await;
    let StatementOutcome::Failed { editor_offset, .. } = outcomes(&evs).pop().unwrap() else {
        panic!("{evs:?}")
    };
    let off = editor_offset.expect("offset");
    assert_eq!(&sql[off..off + 3], "tbl");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safety_rules_refuse_writes_in_read_only_mode() {
    let mut m = Ms::start(ms!(), EnvLabel::Production).await;
    // 本番ラベルは読み取り専用で始まり、書き込みは、1 文も実行されない
    // (拒否するのは core の判定。SQL Server 側の備えは `plan.rs`)
    let evs = m.h.exec(m.tab, "select 1; create table t (a int)").await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    m.h.core.send(Command::SetReadOnly {
        tab: m.tab,
        read_only: false,
    });
    m.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    m.h.exec(
        m.tab,
        "create table t (a int); insert into t values (1), (2)",
    )
    .await;
    // 読み取り専用に戻すと、再び書き込みは拒否される
    m.h.core.send(Command::SetReadOnly {
        tab: m.tab,
        read_only: true,
    });
    m.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: true,
                ..
            }
        )
    })
    .await;
    let evs = m.h.exec(m.tab, "delete from t where a = 1").await;
    assert!(matches!(
        status(&evs),
        ExecutionStatus::RefusedReadOnly { .. }
    ));
    m.h.core.send(Command::SetReadOnly {
        tab: m.tab,
        read_only: false,
    });
    m.h.until(|e| {
        matches!(
            e,
            Event::ReadOnlyChanged {
                read_only: false,
                ..
            }
        )
    })
    .await;
    assert_eq!(m.rows("select count(*) from t").await[0][0], cell("2"));

    // WHERE のない DELETE は、確認が出る。断ると、実行されない
    m.h.send_exec(
        m.tab,
        "delete from t",
        ExecuteTarget::All,
        ExecuteOptions::default(),
    );
    let evs =
        m.h.until(|e| matches!(e, Event::ConfirmationRequired { .. }))
            .await;
    let Event::ConfirmationRequired { id, items, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(items[0].kind, ConfirmKind::UnfilteredDelete);
    m.h.core.send(Command::Confirm { id, accept: false });
    m.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(m.rows("select count(*) from t").await[0][0], cell("2"));

    // トランザクションの状態も追跡できる(BEGIN TRANSACTION と BEGIN TRAN のどちらでも)
    for begin in ["begin transaction", "begin tran"] {
        let evs = m.h.exec(m.tab, begin).await;
        assert!(
            evs.iter().any(|e| matches!(
                e,
                Event::TransactionChanged {
                    state: TxnState::Open,
                    ..
                }
            )),
            "{begin}: {evs:?}"
        );
        m.h.exec(m.tab, "rollback").await;
        assert!(m.h.core.pending_transactions().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn table_editing_with_optimistic_conflict_detection() {
    let mut m = Ms::start(ms!(), EnvLabel::Dev).await;
    m.h.exec(m.tab, "create table people (id int identity(1,1) primary key, name nvarchar(50), note nvarchar(max)); set identity_insert people on; insert into people (id, name, note) values (1, 'Alice', null), (2, 'Bob', 'x'); set identity_insert people off").await;
    m.h.core.send(Command::OpenTable {
        tab: m.tab,
        table: m.table("people"),
    });
    let evs =
        m.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
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
    let prepare = |m: &mut Ms, b: &EditBuffer| {
        m.h.core.send(Command::PrepareEdit {
            tab: m.tab,
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
    prepare(&mut m, &b);
    let evs =
        m.h.until(|e| matches!(e, Event::EditPreview { .. } | Event::CommandFailed { .. }))
            .await;
    let Event::EditPreview {
        plan, statements, ..
    } = evs.last().unwrap().clone()
    else {
        panic!("{evs:?}")
    };
    assert!(
        statements.iter().any(|s| {
            s.sql.contains("[note] IS NULL AND NULL IS NULL") && s.sql.contains("[id] = 1")
        }),
        "{statements:?}"
    );
    m.h.core.send(Command::CommitEdit { tab: m.tab, plan });
    let done =
        m.h.until(|e| matches!(e, Event::EditCommitted { .. } | Event::EditRejected { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(done, Event::EditCommitted { statements: 3, .. }),
        "{done:?}"
    );
    let got = m
        .rows("select id, name, note from people order by id")
        .await;
    assert_eq!(
        got,
        [
            [cell("1"), cell("Alice"), cell("filled")],
            // id を指定しない追加は、IDENTITY が採番する(削除した 2 は再利用されず、3 になる)
            [cell("3"), cell("Carol"), None]
        ]
    );

    // 他の接続が先に更新した行は、上書きしない(全体を取り消す)
    let mut b = EditBuffer::new();
    b.set_cell(0, 1, Some("mine".into()), &orig(0, 1));
    prepare(&mut m, &b);
    let evs = m.h.until(|e| matches!(e, Event::EditPreview { .. })).await;
    let Event::EditPreview { plan, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    m.h.exec(m.tab, "update people set name = 'theirs' where id = 1")
        .await;
    m.h.core.send(Command::CommitEdit { tab: m.tab, plan });
    let done =
        m.h.until(|e| matches!(e, Event::EditCommitted { .. } | Event::EditRejected { .. }))
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
        m.rows("select name from people where id = 1").await[0][0],
        cell("theirs")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_fetch_limit_sorting_and_schema_browsing() {
    let mut m = Ms::start(ms!(), EnvLabel::Dev).await;
    // 取得上限で止まり、続きの取得と並べ替え(全件を受信した後)ができる
    let sql = format!(
        "select g + 1 as x, (g + 1) * 1.5 as y from {} where g < 3000",
        test_sqlserver::numbers(4)
    );
    m.h.send_exec(
        m.tab,
        &sql,
        ExecuteTarget::All,
        ExecuteOptions {
            fetch_limit: FetchLimit::Rows(1000),
            ..Default::default()
        },
    );
    let evs = m.h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let Event::FetchPaused { result, rows: 1000 } = evs.last().unwrap().clone() else {
        panic!("{evs:?}")
    };
    let handle = result_handle(&evs);
    m.h.core.send(Command::FetchMore {
        result,
        more: FetchMore::Rest,
    });
    m.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(handle.row_count(), 3000);
    m.h.core.send(Command::SortResult {
        result,
        column: 0,
        order: SortOrder::Descending,
    });
    m.h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.with_row(0, |r| r.cell(0).map(<[u8]>::to_vec)),
        Access::Ready(Some(b"3000".to_vec()))
    );

    // 実行中のクエリを止められる(終わらない結合)
    let slow = format!(
        "select sum(cast(a.g as bigint) * b.g * c.g) from {}, {}, {}",
        test_sqlserver::numbers_as(4, "a"),
        test_sqlserver::numbers_as(4, "b"),
        test_sqlserver::numbers_as(4, "c"),
    );
    m.h.send_exec(m.tab, &slow, ExecuteTarget::All, ExecuteOptions::default());
    m.h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    m.h.core.send(Command::Cancel { tab: m.tab });
    let evs =
        m.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
            .await;
    assert_eq!(status(&evs), ExecutionStatus::Cancelled);
    // 止めた後も、同じタブで続けて実行できる
    assert_eq!(m.rows("select 'alive'").await[0][0], cell("alive"));

    // スキーマツリー
    m.h.exec(m.tab, "create table parent (id int primary key); create table child (id int primary key, pid int references parent(id))").await;
    let conn = m.h.core_connection_id(m.tab);
    m.h.core.send(Command::LoadSchemas {
        connection_id: conn.clone(),
    });
    let evs = m.h.until(|e| matches!(e, Event::Schemas { .. })).await;
    let Event::Schemas { schemas, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(schemas.iter().any(|s| s.name == "dbo" && !s.system));
    assert!(schemas.iter().any(|s| s.name == "sys" && s.system));
    m.h.core.send(Command::LoadTables {
        connection_id: conn.clone(),
        schema: "dbo".into(),
    });
    let evs = m.h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["child", "parent"]
    );
    m.h.core.send(Command::LoadTableDetails {
        connection_id: conn,
        table: m.table("child"),
    });
    let evs = m.h.until(|e| matches!(e, Event::TableDetails { .. })).await;
    let Event::TableDetails { details, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(details.keys.iter().any(|k| k.kind == KeyKind::Foreign));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn csv_import_and_export_work_on_sql_server() {
    let mut m = Ms::start(ms!(), EnvLabel::Dev).await;
    m.h.exec(
        m.tab,
        "create table items (id int primary key, name nvarchar(50), price decimal(10,2))",
    )
    .await;
    let csv = m.dir.path().join("in.csv");
    std::fs::write(&csv, "id,name,price\n1,商品A,1.5\n2,,2.25\n").unwrap();
    m.h.core.send(Command::ImportCsv {
        tab: m.tab,
        table: m.table("items"),
        path: csv,
        options: CsvReadOptions::default(),
        mapping: vec![(0, "id".into()), (1, "name".into()), (2, "price".into())],
    });
    let done =
        m.h.until(|e| matches!(e, Event::ImportFinished { .. } | Event::ImportFailed { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(done, Event::ImportFinished { rows: 2, .. }),
        "{done:?}"
    );

    let evs = m.h.exec(m.tab, "select * from items order by id").await;
    let r = result_handle(&evs);
    let out = m.dir.path().join("out.csv");
    m.h.core.send(Command::ExportResult {
        result: r.id(),
        path: out.clone(),
        format: ExportFormat::Csv(CsvOptions {
            line_ending: "\n",
            ..Default::default()
        }),
    });
    m.h.until(|e| matches!(e, Event::ExportFinished { .. }))
        .await;
    assert_eq!(
        std::fs::read_to_string(out).unwrap(),
        "id,name,price\n1,商品A,1.50\n2,,2.25\n"
    );
}
