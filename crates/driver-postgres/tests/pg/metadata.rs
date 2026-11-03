//! スキーマ情報の取得。

use std::time::{Duration, Instant};

use driver_api::*;
use driver_postgres::PostgresDriver;

use crate::common::*;

#[tokio::test]
async fn metadata_lists_schemas_tables_columns_keys_and_indexes() {
    let db = pg!();
    let mut s = session(&db).await;
    for sql in [
        "CREATE SCHEMA app",
        "CREATE TABLE app.customers (id bigint PRIMARY KEY, email text NOT NULL UNIQUE, name varchar(20) DEFAULT 'n/a', age int CHECK (age >= 0))",
        "CREATE TABLE app.orders (id serial PRIMARY KEY, customer_id bigint NOT NULL REFERENCES app.customers(id), placed timestamptz)",
        "CREATE INDEX orders_customer_placed ON app.orders (customer_id, placed DESC)",
        "CREATE VIEW app.v_customers AS SELECT id, email FROM app.customers",
        "CREATE MATERIALIZED VIEW app.m_orders AS SELECT * FROM app.orders",
        "CREATE TABLE app.\"Odd \"\"Name\"\"\" (x int)",
    ] {
        run(&mut s, sql).await;
    }
    let meta = PostgresDriver.connect_metadata(&config(&db)).await.unwrap();

    let schemas = meta.schemas().await.unwrap();
    let app = schemas
        .iter()
        .find(|s| s.name == "app")
        .expect("app schema");
    assert!(!app.system);
    assert!(schemas.iter().any(|s| s.name == "pg_catalog" && s.system));
    assert!(schemas.iter().any(|s| s.name == "public"));
    assert!(!schemas.iter().any(|s| s.name.starts_with("pg_toast")));
    let first_system = schemas.iter().position(|s| s.system).unwrap();
    assert!(
        schemas[..first_system].iter().all(|s| !s.system),
        "user schemas come first"
    );

    let tables = meta.tables("app").await.unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert!(kinds.contains(&("customers", TableKind::Table)));
    assert!(kinds.contains(&("v_customers", TableKind::View)));
    assert!(kinds.contains(&("m_orders", TableKind::MaterializedView)));
    assert!(kinds.contains(&("Odd \"Name\"", TableKind::Table)));

    let cols = meta
        .columns(&TableRef::new("app", "customers"))
        .await
        .unwrap();
    assert_eq!(
        cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["id", "email", "name", "age"]
    );
    assert_eq!(cols[0].type_name, "bigint");
    assert!(!cols[0].nullable);
    assert_eq!(cols[2].type_name, "character varying(20)");
    assert!(cols[2].nullable);
    assert_eq!(cols[2].default.as_deref(), Some("'n/a'::character varying"));
    assert_eq!(cols[3].ordinal, 4);

    let keys = meta.keys(&TableRef::new("app", "orders")).await.unwrap();
    let pk = keys.iter().find(|k| k.kind == KeyKind::Primary).unwrap();
    assert_eq!(pk.columns, ["id"]);
    let fk = keys.iter().find(|k| k.kind == KeyKind::Foreign).unwrap();
    assert_eq!(fk.columns, ["customer_id"]);
    assert!(
        fk.definition
            .as_deref()
            .unwrap()
            .contains("REFERENCES app.customers(id)")
    );
    let ckeys = meta.keys(&TableRef::new("app", "customers")).await.unwrap();
    assert!(
        ckeys
            .iter()
            .any(|k| k.kind == KeyKind::Unique && k.columns == ["email"])
    );
    assert!(ckeys.iter().any(|k| k.kind == KeyKind::Check));
    assert_eq!(ckeys[0].kind, KeyKind::Primary, "primary key first");

    let idx = meta.indexes(&TableRef::new("app", "orders")).await.unwrap();
    let composite = idx
        .iter()
        .find(|i| i.name == "orders_customer_placed")
        .unwrap();
    // columns は列(式)だけ。並び順などは definition に含まれる
    assert_eq!(composite.columns, ["customer_id", "placed"]);
    assert!(composite.definition.contains("(customer_id, placed DESC)"));
    assert!(!composite.unique);
    assert_eq!(composite.method, "btree");
    assert!(idx.iter().any(|i| i.unique), "primary key index");

    assert!(
        meta.columns(&TableRef::new("app", "missing"))
            .await
            .unwrap()
            .is_empty()
    );
    meta.close().await.unwrap();
}

#[tokio::test]
async fn metadata_is_not_blocked_by_a_running_query() {
    let db = pg!();
    let mut s = session(&db).await;
    let meta = PostgresDriver.connect_metadata(&config(&db)).await.unwrap();
    let busy = tokio::spawn(async move {
        s.execute(
            &Statement::new("SELECT pg_sleep(2)"),
            &mut Collect::default(),
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    meta.schemas().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "metadata waited {:?}",
        started.elapsed()
    );
    busy.await.unwrap();
}
