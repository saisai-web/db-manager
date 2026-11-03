//! スキーマ情報の取得。

use driver_api::*;
use driver_oracle::OracleDriver;

use crate::common::*;

#[tokio::test]
async fn reads_schemas_tables_columns_keys_and_indexes() {
    let db = ora!();
    let mut s = session(&db).await;
    exec(
        &mut s,
        "CREATE TABLE customers (id NUMBER(19) PRIMARY KEY, name VARCHAR2(50 CHAR) DEFAULT 'x' NOT NULL, email VARCHAR2(80), CONSTRAINT uq_email UNIQUE (email))",
    )
    .await;
    exec(
        &mut s,
        "CREATE TABLE orders (id NUMBER(10) PRIMARY KEY, customer_id NUMBER(19) NOT NULL, total NUMBER(12,2), placed TIMESTAMP(6), \
         CONSTRAINT fk_cust FOREIGN KEY (customer_id) REFERENCES customers(id), CONSTRAINT chk_total CHECK (total >= 0))",
    )
    .await;
    exec(
        &mut s,
        "CREATE INDEX by_customer ON orders (customer_id, total)",
    )
    .await;
    exec(&mut s, "CREATE VIEW v_orders AS SELECT * FROM orders").await;

    let meta = OracleDriver.connect_metadata(&config(&db)).await.unwrap();
    let schemas = meta.schemas().await.unwrap();
    let mine = schemas.iter().find(|x| x.name == db.user).unwrap();
    assert!(!mine.system);
    assert!(schemas.iter().any(|x| x.name == "SYS" && x.system));

    let tables = meta.tables(&db.user).await.unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("CUSTOMERS", TableKind::Table),
            ("ORDERS", TableKind::Table),
            ("V_ORDERS", TableKind::View)
        ]
    );

    let orders = TableRef::new(&db.user, "ORDERS");
    let cols = meta.columns(&orders).await.unwrap();
    let names: Vec<_> = cols
        .iter()
        .map(|c| (c.name.as_str(), c.type_name.as_str(), c.nullable))
        .collect();
    assert_eq!(
        names,
        [
            ("ID", "NUMBER(10)", false),
            ("CUSTOMER_ID", "NUMBER(19)", false),
            ("TOTAL", "NUMBER(12,2)", true),
            ("PLACED", "TIMESTAMP(6)", true)
        ]
    );
    assert_eq!(cols[0].ordinal, 1);
    let cust_cols = meta
        .columns(&TableRef::new(&db.user, "CUSTOMERS"))
        .await
        .unwrap();
    assert_eq!(cust_cols[1].type_name, "VARCHAR2(50 CHAR)");
    assert_eq!(cust_cols[1].default.as_deref(), Some("'x'"));

    let keys = meta.keys(&orders).await.unwrap();
    let pk = keys.iter().find(|k| k.kind == KeyKind::Primary).unwrap();
    assert_eq!(pk.columns, ["ID"]);
    let fk = keys.iter().find(|k| k.kind == KeyKind::Foreign).unwrap();
    assert_eq!(fk.name, "FK_CUST");
    assert_eq!(fk.columns, ["CUSTOMER_ID"]);
    let def = fk.definition.as_deref().unwrap();
    assert!(
        def.contains("REFERENCES") && def.contains("\"CUSTOMERS\" (\"ID\")"),
        "{def}"
    );
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Check && k.name == "CHK_TOTAL"),
        "{keys:?}"
    );
    assert!(
        !keys
            .iter()
            .any(|k| k.kind == KeyKind::Check && k.name.starts_with("SYS_C")),
        "NOT NULL constraints are not listed: {keys:?}"
    );

    let keys = meta
        .keys(&TableRef::new(&db.user, "CUSTOMERS"))
        .await
        .unwrap();
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Unique && k.columns == ["EMAIL"])
    );

    let idx = meta.indexes(&orders).await.unwrap();
    let by = idx.iter().find(|i| i.name == "BY_CUSTOMER").unwrap();
    assert_eq!(by.columns, ["CUSTOMER_ID", "TOTAL"]);
    assert!(!by.unique);
    assert!(
        idx.iter().any(|i| i.unique),
        "the primary key's index is unique"
    );
    meta.close().await.unwrap();
}
