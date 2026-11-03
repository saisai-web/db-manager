//! 取得上限、キャンセル、並べ替え、スキーマツリー、CSV の取り込みと書き出し。

use sqltool_core::*;

use super::My;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_fetch_limit_sorting_and_schema_browsing() {
    let mut m = My::start(my!(), EnvLabel::Dev).await;
    // 取得上限で止まり、続きの取得と並べ替え(全件を受信した後)ができる
    let sql = format!(
        "select g + 1 as x, (g + 1) * 1.5 as y from {} where g < 3000",
        test_mysql::numbers(4)
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
        "select sum(a.g * b.g * c.g) from {}, {}, {}",
        test_mysql::numbers_as(4, "a"),
        test_mysql::numbers_as(4, "b"),
        test_mysql::numbers_as(4, "c"),
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

    // スキーマツリー
    m.h.exec(m.tab, "create table parent (id int primary key); create table child (id int primary key, pid int, foreign key (pid) references parent(id))").await;
    let conn = m.h.core_connection_id(m.tab);
    m.h.core.send(Command::LoadSchemas {
        connection_id: conn.clone(),
    });
    let evs = m.h.until(|e| matches!(e, Event::Schemas { .. })).await;
    let Event::Schemas { schemas, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(schemas.iter().any(|s| s.name == m.db.dbname && !s.system));
    m.h.core.send(Command::LoadTables {
        connection_id: conn.clone(),
        schema: m.db.dbname.clone(),
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
async fn csv_import_and_export_work_on_mysql() {
    let mut m = My::start(my!(), EnvLabel::Dev).await;
    m.h.exec(
        m.tab,
        "create table items (id int primary key, name varchar(50), price decimal(10,2))",
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
