//! ER 設計で作った CREATE 文が、実際の DB で通ることを確かめる(PostgreSQL・SQLite・MySQL・SQL Server・Oracle)。
//! 流したあと、できた表を読み戻して設計に戻し、表・主キー・関係が設計どおりかを見る。
//! DB を用意できなければ、その DB は飛ばす。

use sqltool_core::design::ddl::{DdlOptions, Target, generate};
use sqltool_core::design::dsl;
use sqltool_core::*;

use crate::common::*;

/// 自動採番・既定値・一意制約・索引・複合主キー・自己参照・循環する参照・連鎖削除を含む設計。
const DESIGN: &str = r#"
@title 検証用

customers "顧客" "会員"
  id      int pk auto "顧客ID"
  name    varchar(100) not null "氏名"
  email   varchar(255) not null unique "メール"
  active  bool not null default true "有効"
  joined  datetime not null default now() "登録日時"

categories "分類"
  id        int pk auto
  name      varchar(50) not null
  parent_id int -> categories.id on delete set null

products "商品"
  id          int pk auto
  category_id int not null -> categories.id
  name        varchar(200) not null
  price       decimal(10,2) not null default 0
  note        text
  index (category_id)
  unique uq_products_name (name)

orders "注文"
  id          bigint pk auto
  customer_id int not null -> customers.id on delete cascade "顧客"
  status      char(1) not null default 'N'
  placed      date

order_items "注文明細"
  order_id   bigint pk -> orders.id on delete cascade
  line_no    int pk
  product_id int not null -> products.id
  quantity   int not null default 1

left_side
  id int pk
  right_id int -> right_side.id
right_side
  id int pk
  left_id int -> left_side.id
"#;

/// DROP の確認に答えながら、全文を実行する。
async fn run(h: &mut H, tab: TabId, sql: &str) -> Vec<Event> {
    h.send_exec(tab, sql, ExecuteTarget::All, ExecuteOptions::default());
    let mut all = Vec::new();
    loop {
        let evs = h
            .until(|e| {
                matches!(e, Event::ExecutionFinished { tab: t, .. } if *t == tab)
                    || matches!(
                        e,
                        Event::ConfirmationRequired { .. } | Event::CommandFailed { .. }
                    )
            })
            .await;
        let last = evs.last().unwrap().clone();
        all.extend(evs);
        match last {
            Event::ConfirmationRequired { id, .. } => {
                h.core.send(Command::Confirm { id, accept: true })
            }
            _ => return all,
        }
    }
}

fn assert_all_completed(what: &str, sql: &str, evs: &[Event]) {
    let failed: Vec<_> = outcomes(evs)
        .into_iter()
        .filter(|o| !is_completed(o))
        .collect();
    assert!(
        failed.is_empty() && status(evs) == ExecutionStatus::Completed,
        "{what}: {failed:?}\n--- SQL ---\n{sql}\n--- events ---\n{evs:?}"
    );
}

/// スキーマの表をすべて読み戻して、設計にする。
async fn read_back(h: &mut H, tab: TabId, schema: &str) -> sqltool_core::design::Design {
    let conn = h.core_connection_id(tab);
    h.core.send(Command::LoadTables {
        connection_id: conn.clone(),
        schema: schema.into(),
    });
    let evs = h.until(|e| matches!(e, Event::Tables { .. })).await;
    let Event::Tables { tables, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    let mut details = Vec::new();
    for t in tables.iter().filter(|t| t.kind == TableKind::Table) {
        h.core.send(Command::LoadTableDetails {
            connection_id: conn.clone(),
            table: TableRef::new(&t.schema, &t.name),
        });
        let evs = h.until(|e| matches!(e, Event::TableDetails { .. })).await;
        let Event::TableDetails { details: d, .. } = evs.last().unwrap().clone() else {
            unreachable!()
        };
        details.push(d);
    }
    design_from_details("read back", &details)
}

/// 設計を流して、読み戻した結果を、もとの設計と比べる。
async fn check(h: &mut H, tab: TabId, target: Target, schema: &str, opts: DdlOptions) {
    let design = dsl::parse(DESIGN).unwrap();
    let ddl = generate(
        &design,
        target,
        &DdlOptions {
            drop_first: true,
            ..opts
        },
    );
    assert_all_completed(target.label(), &ddl.sql, &run(h, tab, &ddl.sql).await);
    // DROP つきなので、もう一度流しても通る
    assert_all_completed(
        &format!("{} (again)", target.label()),
        &ddl.sql,
        &run(h, tab, &ddl.sql).await,
    );

    let back = read_back(h, tab, schema).await;
    let names = |d: &sqltool_core::design::Design| {
        let mut v: Vec<_> = d.tables.iter().map(|t| t.name.to_lowercase()).collect();
        v.sort();
        v
    };
    assert_eq!(names(&back), names(&design), "{}", target.label());
    for want in &design.tables {
        let got = back
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(&want.name))
            .unwrap();
        let lower = |v: Vec<String>| v.into_iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        assert_eq!(
            lower(got.primary_key()),
            lower(want.primary_key()),
            "{}: primary key of {}",
            target.label(),
            want.name
        );
        assert_eq!(
            got.columns.len(),
            want.columns.len(),
            "{}: columns of {}",
            target.label(),
            want.name
        );
        let fks = |t: &sqltool_core::design::Table| {
            let mut v: Vec<_> = t
                .foreign_keys
                .iter()
                .map(|f| (lower(f.columns.clone()), f.ref_table.to_lowercase()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            fks(got),
            fks(want),
            "{}: foreign keys of {}",
            target.label(),
            want.name
        );
    }
    // 連鎖削除が、DB に実際に効いていること
    let evs = run(
        h,
        tab,
        "insert into customers (name, email) values ('a', 'a@example.com'); \
         insert into orders (customer_id) select min(id) from customers; \
         delete from customers; \
         select count(*) from orders",
    )
    .await;
    let r = result_handle(&evs);
    assert_eq!(
        rows(&r),
        [[cell("0")]],
        "{}: ON DELETE CASCADE",
        target.label()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_generated_sql_runs_on_sqlite() {
    let mut h = H::start_sqlite().await;
    let dir = tempfile::tempdir().unwrap();
    let tab = h
        .open_sqlite_tab(&dir.path().join("d.db"), EnvLabel::Dev)
        .await;
    check(&mut h, tab, Target::Sqlite, "main", DdlOptions::default()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_generated_sql_runs_on_postgresql() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    check(
        &mut h,
        tab,
        Target::Postgres,
        "public",
        DdlOptions::default(),
    )
    .await;
    // 索引・コメントなどの付き方を変えても通る
    let opts = DdlOptions {
        comments: false,
        index_foreign_keys: true,
        ..Default::default()
    };
    check(&mut h, tab, Target::Postgres, "public", opts).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_generated_sql_runs_on_mysql() {
    let Some(db) = test_mysql::database_or_skip() else {
        return;
    };
    let mut h = H::start_without_pg().await;
    let tab = h
        .open_net_tab(
            "mysql",
            NetConn {
                host: &db.host,
                port: db.port,
                user: &db.user,
                password: db.password.as_deref(),
                database: &db.dbname,
            },
            EnvLabel::Dev,
        )
        .await;
    check(
        &mut h,
        tab,
        Target::MySql,
        &db.dbname,
        DdlOptions::default(),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_generated_sql_runs_on_sql_server() {
    let Some(db) = test_sqlserver::database_or_skip() else {
        return;
    };
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
            EnvLabel::Dev,
        )
        .await;
    check(&mut h, tab, Target::SqlServer, "dbo", DdlOptions::default()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_generated_sql_runs_on_oracle() {
    let Some(db) = test_oracle::database_or_skip() else {
        return;
    };
    let mut h = H::start_without_pg().await;
    let tab = h
        .open_net_tab(
            "oracle",
            NetConn {
                host: &db.host,
                port: db.port,
                user: &db.user,
                password: Some(&db.password),
                database: &db.service,
            },
            EnvLabel::Dev,
        )
        .await;
    check(&mut h, tab, Target::Oracle, &db.user, DdlOptions::default()).await;
}
