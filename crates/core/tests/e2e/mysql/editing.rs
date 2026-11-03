//! テーブルの編集(楽観的ロックによる競合の検出)。

use sqltool_core::*;

use super::My;
use crate::common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn table_editing_with_optimistic_conflict_detection() {
    let mut m = My::start(my!(), EnvLabel::Dev).await;
    m.h.exec(m.tab, "create table people (id int auto_increment primary key, name varchar(50), note text); insert into people values (1, 'Alice', null), (2, 'Bob', 'x')").await;
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
    let prepare = |m: &mut My, b: &EditBuffer| {
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
        statements
            .iter()
            .any(|s| s.sql.contains("`note` <=> NULL") && s.sql.contains("`id` = 1")),
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
            // id を指定しない追加は、AUTO_INCREMENT が採番する(削除した 2 は再利用されず、3 になる)
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
