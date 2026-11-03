//! 取得上限、並べ替え、キャンセル、スキーマツリー、CSV の取り込みと書き出し。

use sqltool_core::*;

use super::Ora;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_limit_sorting_cancel_and_schema_browsing() {
    let db = ora!();
    // 実行中の文を止めるには、ALTER SYSTEM 権限が要る
    test_oracle::admin(&format!("GRANT ALTER SYSTEM TO \"{}\"", db.user)).unwrap();
    let mut o = Ora::start(db, EnvLabel::Dev).await;
    // 取得上限で止まり、続きの取得と並べ替え(全件を受信した後)ができる
    o.h.send_exec(
        o.tab,
        "select level as x, level * 1.5 as y from dual connect by level <= 3000",
        ExecuteTarget::All,
        ExecuteOptions {
            fetch_limit: FetchLimit::Rows(1000),
            ..Default::default()
        },
    );
    let evs = o.h.until(|e| matches!(e, Event::FetchPaused { .. })).await;
    let Event::FetchPaused { result, rows: 1000 } = evs.last().unwrap().clone() else {
        panic!("{evs:?}")
    };
    let handle = result_handle(&evs);
    o.h.core.send(Command::FetchMore {
        result,
        more: FetchMore::Rest,
    });
    o.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
        .await;
    assert_eq!(handle.row_count(), 3000);
    o.h.core.send(Command::SortResult {
        result,
        column: 0,
        order: SortOrder::Descending,
    });
    o.h.until(|e| matches!(e, Event::SortChanged { .. })).await;
    assert_eq!(
        handle.with_row(0, |r| r.cell(0).map(<[u8]>::to_vec)),
        Access::Ready(Some(b"3000".to_vec()))
    );

    // スキーマツリー
    o.h.exec(o.tab, "create table parent (id number primary key); create table child (id number primary key, pid number references parent(id))").await;
    let conn = o.h.core_connection_id(o.tab);
    o.h.core.send(Command::LoadSchemas {
        connection_id: conn.clone(),
    });
    let evs = o.h.until(|e| matches!(e, Event::Schemas { .. })).await;
    let Event::Schemas { schemas, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(schemas.iter().any(|s| s.name == o.db.user && !s.system));
    o.h.core.send(Command::LoadTables {
        connection_id: conn.clone(),
        schema: o.db.user.clone(),
    });
    let evs = o.h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert_eq!(
        tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["CHILD", "PARENT"]
    );
    o.h.core.send(Command::LoadTableDetails {
        connection_id: conn,
        table: o.table("CHILD"),
    });
    let evs = o.h.until(|e| matches!(e, Event::TableDetails { .. })).await;
    let Event::TableDetails { details, .. } = evs.last().unwrap() else {
        unreachable!()
    };
    assert!(details.keys.iter().any(|k| k.kind == KeyKind::Foreign));

    // 実行中のクエリを止められる(セッションごと終了する。以降はこのタブを開き直す)
    let slow = "select count(*) from (select level a from dual connect by level <= 3000) x, \
                (select level b from dual connect by level <= 3000) y, \
                (select level c from dual connect by level <= 3000) z where mod(a * b * c, 7) = 3";
    o.h.send_exec(o.tab, slow, ExecuteTarget::All, ExecuteOptions::default());
    o.h.until(|e| matches!(e, Event::StatementStarted { .. }))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    o.h.core.send(Command::Cancel { tab: o.tab });
    let evs =
        o.h.until(|e| matches!(e, Event::ExecutionFinished { .. }))
            .await;
    assert_eq!(status(&evs), ExecutionStatus::Cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn csv_import_and_export_work_on_oracle() {
    let mut o = Ora::start(ora!(), EnvLabel::Dev).await;
    o.h.exec(
        o.tab,
        "create table items (id number primary key, name varchar2(50), price number(10,2))",
    )
    .await;
    let csv = o.dir.path().join("in.csv");
    std::fs::write(&csv, "id,name,price\n1,商品A,1.5\n2,,2.25\n").unwrap();
    o.h.core.send(Command::ImportCsv {
        tab: o.tab,
        table: o.table("ITEMS"),
        path: csv,
        options: CsvReadOptions::default(),
        mapping: vec![(0, "ID".into()), (1, "NAME".into()), (2, "PRICE".into())],
    });
    let done =
        o.h.until(|e| matches!(e, Event::ImportFinished { .. } | Event::ImportFailed { .. }))
            .await
            .pop()
            .unwrap();
    assert!(
        matches!(done, Event::ImportFinished { rows: 2, .. }),
        "{done:?}"
    );

    let evs = o.h.exec(o.tab, "select * from items order by id").await;
    let r = result_handle(&evs);
    let out = o.dir.path().join("out.csv");
    o.h.core.send(Command::ExportResult {
        result: r.id(),
        path: out.clone(),
        format: ExportFormat::Csv(CsvOptions {
            line_ending: "\n",
            ..Default::default()
        }),
    });
    o.h.until(|e| matches!(e, Event::ExportFinished { .. }))
        .await;
    // NUMBER は、末尾の 0 を持たない(Oracle が保持する値のまま)
    assert_eq!(
        std::fs::read_to_string(out).unwrap(),
        "ID,NAME,PRICE\n1,商品A,1.5\n2,,2.25\n"
    );
}
