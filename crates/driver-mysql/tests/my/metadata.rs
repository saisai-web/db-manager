//! スキーマ情報の取得。

use driver_api::*;
use driver_mysql::MysqlDriver;

use crate::common::*;

#[tokio::test]
async fn reads_schemas_tables_columns_keys_and_indexes() {
    let db = my!();
    let mut s = session(&db).await;
    run(
        &mut s,
        "CREATE TABLE customers (id bigint unsigned PRIMARY KEY, name varchar(50) NOT NULL DEFAULT 'x', email varchar(80), UNIQUE KEY uq_email (email))",
    )
    .await;
    run(
        &mut s,
        "CREATE TABLE orders (id int AUTO_INCREMENT PRIMARY KEY, customer_id bigint unsigned NOT NULL, total decimal(12,2), \
         CONSTRAINT fk_cust FOREIGN KEY (customer_id) REFERENCES customers(id), CONSTRAINT chk_total CHECK (total >= 0), \
         KEY by_customer (customer_id, total))",
    )
    .await;
    run(&mut s, "CREATE VIEW v_orders AS SELECT * FROM orders").await;

    let meta = MysqlDriver.connect_metadata(&config(&db)).await.unwrap();
    let schemas = meta.schemas().await.unwrap();
    let mine = schemas.iter().find(|x| x.name == db.dbname).unwrap();
    assert!(!mine.system);
    assert!(schemas.iter().any(|x| x.name == "mysql" && x.system));

    let tables = meta.tables(&db.dbname).await.unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("customers", TableKind::Table),
            ("orders", TableKind::Table),
            ("v_orders", TableKind::View)
        ]
    );

    let orders = TableRef::new(&db.dbname, "orders");
    let cols = meta.columns(&orders).await.unwrap();
    // MariaDB は整数に表示幅を付けて返す(`int(11)`)。DB の表記のまま出すので、比べるときだけ除く
    let names: Vec<(String, String, bool)> = cols
        .iter()
        .map(|c| {
            let t = match c.type_name.split_once('(') {
                Some((head, tail)) if head.ends_with("int") => {
                    format!("{head}{}", tail.split_once(')').map_or("", |(_, r)| r))
                }
                _ => c.type_name.clone(),
            };
            (c.name.clone(), t, c.nullable)
        })
        .collect();
    let names: Vec<_> = names
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), *c))
        .collect();
    assert_eq!(
        names,
        [
            ("id", "int", false),
            ("customer_id", "bigint unsigned", false),
            ("total", "decimal(12,2)", true)
        ]
    );
    assert_eq!(cols[0].ordinal, 1);

    let keys = meta.keys(&orders).await.unwrap();
    let pk = keys.iter().find(|k| k.kind == KeyKind::Primary).unwrap();
    assert_eq!(pk.columns, ["id"]);
    let fk = keys.iter().find(|k| k.kind == KeyKind::Foreign).unwrap();
    assert_eq!(fk.name, "fk_cust");
    assert_eq!(fk.columns, ["customer_id"]);
    let def = fk.definition.as_deref().unwrap();
    assert!(
        def.contains("REFERENCES") && def.contains("`customers` (`id`)"),
        "{def}"
    );
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Check && k.name == "chk_total"),
        "{keys:?}"
    );

    let customers = TableRef::new(&db.dbname, "customers");
    let keys = meta.keys(&customers).await.unwrap();
    assert!(
        keys.iter()
            .any(|k| k.kind == KeyKind::Unique && k.columns == ["email"])
    );

    let idx = meta.indexes(&orders).await.unwrap();
    let by = idx.iter().find(|i| i.name == "by_customer").unwrap();
    assert_eq!(by.columns, ["customer_id", "total"]);
    assert!(!by.unique);
    assert!(idx.iter().any(|i| i.name == "PRIMARY" && i.unique));
    meta.close().await.unwrap();
}
