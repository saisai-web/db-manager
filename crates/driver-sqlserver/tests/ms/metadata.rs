//! スキーマ情報の取得。

use driver_api::*;
use driver_sqlserver::SqlServerDriver;

use crate::common::*;

#[tokio::test]
async fn reads_schemas_tables_columns_keys_and_indexes() {
    let db = ms!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE TABLE customers (id bigint NOT NULL CONSTRAINT pk_customers PRIMARY KEY, \
         name nvarchar(50) NOT NULL CONSTRAINT df_name DEFAULT N'x', email varchar(80) NULL, \
         CONSTRAINT uq_email UNIQUE (email))",
    )
    .await;
    run(
        &mut s,
        "CREATE TABLE orders (id int IDENTITY(1,1) CONSTRAINT pk_orders PRIMARY KEY, customer_id bigint NOT NULL, \
         total decimal(12,2) NULL, at datetime2(3) NULL, \
         CONSTRAINT fk_cust FOREIGN KEY (customer_id) REFERENCES customers(id), \
         CONSTRAINT chk_total CHECK (total >= 0))",
    )
    .await;
    run(
        &mut s,
        "CREATE INDEX by_customer ON orders (customer_id, total)",
    )
    .await;
    run(
        &mut s,
        "CREATE VIEW v_orders AS SELECT id, total FROM orders",
    )
    .await;

    let meta = SqlServerDriver
        .connect_metadata(&config(&db))
        .await
        .unwrap();
    let schemas = meta.schemas().await.unwrap();
    let dbo = schemas.iter().find(|x| x.name == "dbo").unwrap();
    assert!(!dbo.system);
    assert!(schemas.iter().any(|x| x.name == "sys" && x.system));

    let tables = meta.tables("dbo").await.unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("customers", TableKind::Table),
            ("orders", TableKind::Table),
            ("v_orders", TableKind::View)
        ]
    );

    let orders = TableRef::new("dbo", "orders");
    let cols = meta.columns(&orders).await.unwrap();
    let names: Vec<_> = cols
        .iter()
        .map(|c| (c.name.as_str(), c.type_name.as_str(), c.nullable))
        .collect();
    assert_eq!(
        names,
        [
            ("id", "int", false),
            ("customer_id", "bigint", false),
            ("total", "decimal(12,2)", true),
            ("at", "datetime2(3)", true)
        ]
    );
    assert_eq!(cols[0].ordinal, 1);
    let cust = meta
        .columns(&TableRef::new("dbo", "customers"))
        .await
        .unwrap();
    assert_eq!(cust[1].type_name, "nvarchar(50)");
    assert_eq!(cust[1].default.as_deref(), Some("(N'x')"));
    assert_eq!(cust[2].type_name, "varchar(80)");

    let keys = meta.keys(&orders).await.unwrap();
    let pk = keys.iter().find(|k| k.kind == KeyKind::Primary).unwrap();
    assert_eq!(pk.columns, ["id"]);
    let fk = keys.iter().find(|k| k.kind == KeyKind::Foreign).unwrap();
    assert_eq!(fk.name, "fk_cust");
    assert_eq!(fk.columns, ["customer_id"]);
    assert_eq!(
        fk.definition.as_deref(),
        Some("FOREIGN KEY ([customer_id]) REFERENCES [dbo].[customers] ([id])")
    );
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Check && k.name == "chk_total"),
        "{keys:?}"
    );

    let keys = meta.keys(&TableRef::new("dbo", "customers")).await.unwrap();
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Unique && k.columns == ["email"])
    );

    let idx = meta.indexes(&orders).await.unwrap();
    let by = idx.iter().find(|i| i.name == "by_customer").unwrap();
    assert_eq!(by.columns, ["customer_id", "total"]);
    assert!(!by.unique);
    assert!(idx.iter().any(|i| i.name == "pk_orders" && i.unique));
    meta.close().await.unwrap();
}
