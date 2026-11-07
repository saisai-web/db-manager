//! テーブル編集: 開く、変更、プレビュー、保存(1 つのトランザクション、楽観的な競合の検出)。

use sqltool_core::*;

use crate::common::*;

const SETUP: &str = "
create table customers (id bigint primary key, name text not null, email text, balance numeric(20,6), joined timestamptz, flag boolean, bin bytea);
insert into customers values
 (1, 'Alice', 'a@example.com', 1.50, '2026-01-02 03:04:05.123456+09', true, '\\xdead'),
 (2, '佐伯 光平', null, 12345678901234.123456, null, false, null),
 (3, '', '', 0, null, null, null),
 (4, 'Dave', 'd@example.com', -5, null, null, null);
create table keyless (a int, b text);
insert into keyless values (1, 'x');
create view v_customers as select id, name from customers;
";

struct Editing {
    h: H,
    tab: TabId,
}

impl Editing {
    async fn start() -> Option<Self> {
        let db = test_pg::database_or_skip()?;
        let mut h = H::start(db).await;
        let tab = h.open_tab(EnvLabel::Dev).await;
        h.exec(tab, SETUP).await;
        Some(Self { h, tab })
    }

    /// テーブルを開き、結果を返す。
    async fn open(&mut self, name: &str) -> ResultHandle {
        self.h.core.send(Command::OpenTable {
            tab: self.tab,
            table: TableRef::new("public", name),
            limit: None,
        });
        let evs = self
            .h
            .until(|e| {
                matches!(
                    e,
                    Event::ExecutionFinished { .. } | Event::CommandFailed { .. }
                )
            })
            .await;
        result_handle(&evs)
    }

    fn changes(handle: &ResultHandle, buffer: &EditBuffer) -> Vec<Change> {
        let EditAccess::Editable(source) = handle.edit_access() else {
            panic!("not editable: {:?}", handle.edit_access())
        };
        let names: Vec<String> = handle.columns().iter().map(|c| c.name.clone()).collect();
        buffer.to_changes(source, &names, |row, col| {
            handle
                .read_row_blocking(row)
                .unwrap()
                .unwrap()
                .swap_remove(col)
                .map(|b| String::from_utf8(b).unwrap())
        })
    }

    /// 変更をプレビューして、プレビューの文を返す。
    async fn prepare(
        &mut self,
        handle: &ResultHandle,
        buffer: &EditBuffer,
    ) -> (PlanId, Vec<EditPreviewItem>) {
        self.h.core.send(Command::PrepareEdit {
            tab: self.tab,
            result: handle.id(),
            changes: Self::changes(handle, buffer),
        });
        let evs = self
            .h
            .until(|e| matches!(e, Event::EditPreview { .. } | Event::CommandFailed { .. }))
            .await;
        match evs.last().unwrap() {
            Event::EditPreview {
                plan, statements, ..
            } => (*plan, statements.clone()),
            other => panic!("{other:?}"),
        }
    }

    async fn commit(&mut self, plan: PlanId) -> Event {
        self.h.core.send(Command::CommitEdit {
            tab: self.tab,
            plan,
        });
        self.h
            .until(|e| {
                matches!(
                    e,
                    Event::EditCommitted { .. }
                        | Event::EditRejected { .. }
                        | Event::CommandFailed { .. }
                )
            })
            .await
            .pop()
            .unwrap()
    }

    /// 全行を文字列で読む(NULL は `None`)。
    async fn table_rows(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let evs = self.h.exec(self.tab, sql).await;
        rows(&result_handle(&evs))
    }
}

fn value(s: &str) -> Value {
    Some(s.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_tables_with_a_row_identifying_key_are_editable() {
    let Some(mut e) = Editing::start().await else {
        return;
    };
    let customers = e.open("customers").await;
    let EditAccess::Editable(src) = customers.edit_access() else {
        panic!("{:?}", customers.edit_access())
    };
    assert_eq!(src.key, ["id"]);
    assert_eq!(customers.row_count(), 4);

    for name in ["keyless", "v_customers"] {
        let r = e.open(name).await;
        assert!(
            matches!(r.edit_access(), EditAccess::ReadOnly(_)),
            "{name}: {:?}",
            r.edit_access()
        );
    }
    // 任意の SQL の結果は、編集できない
    let evs = e.h.exec(e.tab, "select * from customers").await;
    assert_eq!(*result_handle(&evs).edit_access(), EditAccess::NotATable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editing_updates_inserts_and_deletes_in_one_transaction_with_exact_values() {
    let Some(mut e) = Editing::start().await else {
        return;
    };
    let r = e.open("customers").await;
    let mut b = EditBuffer::new();
    let orig = |row: u64, col: usize| {
        r.read_row_blocking(row).unwrap().unwrap()[col]
            .clone()
            .map(|v| String::from_utf8(v).unwrap())
    };
    // 行 0(id=1): 名前を変え、メールを NULL にし、bytea と日時と bool も変える
    b.set_cell(0, 1, value("O'Brien \"quoted\" 日本語"), &orig(0, 1));
    b.set_cell(0, 2, None, &orig(0, 2));
    b.set_cell(0, 4, value("2026-11-02 12:34:56.654321+09"), &orig(0, 4));
    b.set_cell(0, 5, value("false"), &orig(0, 5));
    b.set_cell(0, 6, value("\\xcafe"), &orig(0, 6));
    // 行 1(id=2): 桁数の多い numeric を変える。NULL だったメールを、空文字列にする
    b.set_cell(1, 3, value("98765432109876.543210"), &orig(1, 3));
    b.set_cell(1, 2, value(""), &orig(1, 2));
    // 行 3(id=4)を削除。新しい行を追加
    b.delete_row(3);
    let ix = b.add_insert(7);
    b.set_insert_cell(ix, 0, Some(value("5")));
    b.set_insert_cell(ix, 1, Some(value("Eve")));
    b.set_insert_cell(ix, 2, Some(None));

    let (plan, preview) = e.prepare(&r, &b).await;
    let kinds: Vec<_> = preview.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [
            ChangeKind::Delete,
            ChangeKind::Update,
            ChangeKind::Update,
            ChangeKind::Insert
        ]
    );
    assert!(
        preview[0]
            .sql
            .starts_with("DELETE FROM \"public\".\"customers\" WHERE \"id\" = 4")
    );
    assert!(
        preview[1]
            .sql
            .contains("SET \"name\" = 'O''Brien \"quoted\" 日本語'"),
        "{}",
        preview[1].sql
    );
    assert!(
        preview[1]
            .sql
            .contains("\"email\" IS NOT DISTINCT FROM 'a@example.com'"),
        "optimistic condition"
    );
    // プレビューだけでは、DB は変わっていない
    assert_eq!(
        e.table_rows("select count(*) from customers").await[0][0],
        value("4")
    );

    let done = e.commit(plan).await;
    assert!(
        matches!(done, Event::EditCommitted { statements: 4, .. }),
        "{done:?}"
    );

    e.h.exec(e.tab, "set timezone = 'Asia/Tokyo'").await;
    let got = e.table_rows("select id, name, email, balance, joined, flag, encode(bin, 'hex') from customers order by id").await;
    assert_eq!(got.len(), 4, "id=4 deleted, id=5 added");
    assert_eq!(
        got[0],
        [
            value("1"),
            value("O'Brien \"quoted\" 日本語"),
            None,
            value("1.500000"),
            value("2026-11-02 12:34:56.654321+09"),
            value("f"),
            value("cafe")
        ]
    );
    assert_eq!(
        got[1][2],
        value(""),
        "NULL edited to an empty string stays an empty string"
    );
    assert_eq!(
        got[1][3],
        value("98765432109876.543210"),
        "no float rounding"
    );
    assert_eq!(
        got[2][..3],
        [value("3"), value(""), value("")],
        "untouched rows are untouched"
    );
    assert_eq!(
        got[3],
        [value("5"), value("Eve"), None, None, None, None, None]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_changed_by_someone_else_is_not_overwritten_and_nothing_is_saved() {
    let Some(mut e) = Editing::start().await else {
        return;
    };
    let r = e.open("customers").await;
    let orig = |row: u64, col: usize| {
        r.read_row_blocking(row).unwrap().unwrap()[col]
            .clone()
            .map(|v| String::from_utf8(v).unwrap())
    };
    let mut b = EditBuffer::new();
    b.set_cell(2, 3, value("777"), &orig(2, 3)); // id=3 は、他の人に先に変えられる
    b.set_cell(3, 1, value("Changed by me"), &orig(3, 1)); // id=4。こちらは問題なく保存できるはず
    let (plan, _) = e.prepare(&r, &b).await;

    // 別の接続で、同じセルを先に更新する
    let other = e.h.open_tab(EnvLabel::Dev).await;
    e.h.exec(other, "update customers set balance = 1 where id = 3")
        .await;

    let done = e.commit(plan).await;
    let Event::EditRejected {
        reason: EditRejection::RowCount { actual, sql, .. },
        ..
    } = done
    else {
        panic!("{done:?}")
    };
    assert_eq!(actual, 0);
    assert!(
        sql.contains("\"id\" = 3"),
        "names the statement that conflicted: {sql}"
    );
    let got = e
        .table_rows("select id, name, balance from customers where id in (3, 4) order by id")
        .await;
    assert_eq!(
        got[0][2],
        value("1.000000"),
        "the other person's value survives"
    );
    assert_eq!(
        got[1][1],
        value("Dave"),
        "the other statement was rolled back too"
    );
    // 取り消したので、タブは通常どおり使える
    assert!(e.h.core.pending_transactions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_database_error_rolls_everything_back() {
    let Some(mut e) = Editing::start().await else {
        return;
    };
    let r = e.open("customers").await;
    let orig = |row: u64, col: usize| {
        r.read_row_blocking(row).unwrap().unwrap()[col]
            .clone()
            .map(|v| String::from_utf8(v).unwrap())
    };
    let mut b = EditBuffer::new();
    b.set_cell(0, 1, value("fine"), &orig(0, 1));
    b.set_cell(1, 1, None, &orig(1, 1)); // name は NOT NULL
    let (plan, _) = e.prepare(&r, &b).await;
    let done = e.commit(plan).await;
    let Event::EditRejected {
        reason: EditRejection::Database {
            statement_index,
            error,
        },
        ..
    } = done
    else {
        panic!("{done:?}")
    };
    assert_eq!(statement_index, 1);
    assert_eq!(error.code.as_deref(), Some("23502"));
    let got = e
        .table_rows("select name from customers where id = 1")
        .await;
    assert_eq!(got[0][0], value("Alice"));
    assert!(e.h.core.pending_transactions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editing_is_refused_when_it_is_unsafe() {
    let Some(mut e) = Editing::start().await else {
        return;
    };
    let r = e.open("customers").await;
    let mut b = EditBuffer::new();
    b.delete_row(0);
    let changes = Editing::changes(&r, &b);

    // 未コミットのトランザクションの中では、保存できない
    e.h.exec(e.tab, "begin").await;
    e.h.core.send(Command::PrepareEdit {
        tab: e.tab,
        result: r.id(),
        changes: changes.clone(),
    });
    e.h.until(|ev| {
        matches!(
            ev,
            Event::CommandFailed {
                command: "PrepareEdit",
                ..
            }
        )
    })
    .await;
    e.h.exec(e.tab, "rollback").await;

    // 読み取り専用モードでは、保存できない
    e.h.core.send(Command::SetReadOnly {
        tab: e.tab,
        read_only: true,
    });
    e.h.until(|ev| matches!(ev, Event::ReadOnlyChanged { .. }))
        .await;
    e.h.core.send(Command::PrepareEdit {
        tab: e.tab,
        result: r.id(),
        changes: changes.clone(),
    });
    e.h.until(|ev| {
        matches!(
            ev,
            Event::CommandFailed {
                command: "PrepareEdit",
                ..
            }
        )
    })
    .await;
    e.h.core.send(Command::SetReadOnly {
        tab: e.tab,
        read_only: false,
    });
    e.h.until(|ev| matches!(ev, Event::ReadOnlyChanged { .. }))
        .await;

    // 任意の SQL の結果や、キーのないテーブルは、編集できない
    let evs = e.h.exec(e.tab, "select * from customers").await;
    let plain = result_handle(&evs);
    e.h.core.send(Command::PrepareEdit {
        tab: e.tab,
        result: plain.id(),
        changes: changes.clone(),
    });
    e.h.until(|ev| {
        matches!(
            ev,
            Event::CommandFailed {
                command: "PrepareEdit",
                ..
            }
        )
    })
    .await;

    // 変更がなければ、プレビューも作らない
    e.h.core.send(Command::PrepareEdit {
        tab: e.tab,
        result: r.id(),
        changes: vec![],
    });
    e.h.until(|ev| {
        matches!(
            ev,
            Event::CommandFailed {
                command: "PrepareEdit",
                ..
            }
        )
    })
    .await;

    // 破棄した計画は、保存できない
    let (plan, _) = e.prepare(&r, &b).await;
    e.h.core.send(Command::DiscardEdit { plan });
    let done = e.commit(plan).await;
    assert!(
        matches!(
            done,
            Event::CommandFailed {
                command: "CommitEdit",
                ..
            }
        ),
        "{done:?}"
    );
    assert_eq!(
        e.table_rows("select count(*) from customers").await[0][0],
        value("4")
    );
}
