//! 読み取り専用モード: 書き込みの文が実行されても、終わりに取り消される。

use std::time::Duration;

use driver_api::*;

use crate::common::*;

async fn count(s: &mut Box<dyn Session>, table: &str) -> String {
    let (_, r) = run(s, &format!("SELECT count(*) FROM {table}")).await;
    r.text(0, 0).unwrap()
}

async fn table_exists(s: &mut Box<dyn Session>, name: &str) -> bool {
    let (_, r) = run(
        s,
        &format!("SELECT count(*) FROM sys.tables WHERE name = '{name}'"),
    )
    .await;
    r.text(0, 0).as_deref() == Some("1")
}

#[tokio::test]
async fn writes_and_ddl_are_undone_while_reads_work() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
    s.set_read_only(true).await.unwrap();

    let (sum, _) = run(&mut s, "INSERT INTO t VALUES (2), (3)").await;
    assert_eq!(sum.affected_rows, Some(2), "the statement itself ran");
    run(&mut s, "UPDATE t SET id = 100").await;
    run(&mut s, "CREATE TABLE made (a int)").await;
    run(&mut s, "DROP TABLE t").await;
    // 読み取りは普通にできる(結果も返る)
    assert_eq!(count(&mut s, "t").await, "1");
    assert!(!table_exists(&mut s, "made").await);
    let (_, r) = run(&mut s, "SELECT id FROM t").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("1"), "UPDATE was undone");

    // 戻すと、書き込みは残る
    s.set_read_only(false).await.unwrap();
    run(&mut s, "INSERT INTO t VALUES (2)").await;
    assert_eq!(count(&mut s, "t").await, "2");
}

#[tokio::test]
async fn an_error_in_a_statement_leaves_no_open_transaction() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int primary key)").await;
    s.set_read_only(true).await.unwrap();
    assert!(
        s.execute(
            &Statement::new("INSERT INTO t VALUES (1); INSERT INTO t VALUES (1)"),
            &mut Collect::default()
        )
        .await
        .is_err()
    );
    // 調べる文自身も包まれるので、モードを戻してから数える
    s.set_read_only(false).await.unwrap();
    let (_, r) = run(&mut s, "SELECT @@TRANCOUNT").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("0"));
    assert_eq!(
        count(&mut s, "t").await,
        "0",
        "the first INSERT was undone too"
    );
}

#[tokio::test]
async fn a_users_transaction_is_kept_until_the_end_and_commit_becomes_a_rollback() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    s.set_read_only(true).await.unwrap();
    run(&mut s, "BEGIN TRANSACTION").await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
    assert_eq!(
        count(&mut s, "t").await,
        "1",
        "visible inside the transaction"
    );
    run(&mut s, "COMMIT").await;
    assert_eq!(count(&mut s, "t").await, "0", "COMMIT did not commit");
    // 取り消した後は、また文ごとに包まれる
    run(&mut s, "INSERT INTO t VALUES (2)").await;
    assert_eq!(count(&mut s, "t").await, "0");
    run(&mut s, "BEGIN TRAN").await;
    run(&mut s, "INSERT INTO t VALUES (3)").await;
    run(&mut s, "ROLLBACK").await;
    // 調べる文自身も包まれるので、モードを戻してから数える
    s.set_read_only(false).await.unwrap();
    let (_, r) = run(&mut s, "SELECT @@TRANCOUNT").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("0"));
}

#[tokio::test]
async fn a_transaction_opened_before_the_switch_is_the_users() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    run(&mut s, "BEGIN TRANSACTION").await;
    run(&mut s, "INSERT INTO t VALUES (1)").await;
    s.set_read_only(true).await.unwrap();
    // 包みで取り消さず、利用者のトランザクションを壊さない
    assert_eq!(count(&mut s, "t").await, "1");
    run(&mut s, "ROLLBACK").await;
    assert_eq!(count(&mut s, "t").await, "0");
}

#[tokio::test]
async fn the_mode_survives_a_cancel_that_reconnects() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    s.set_read_only(true).await.unwrap();
    let canceller = s.canceller();
    let task = tokio::spawn(async move {
        let r = s
            .execute(
                &Statement::new("WAITFOR DELAY '00:00:30'"),
                &mut Collect::default(),
            )
            .await;
        (s, r)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    canceller.cancel().await.unwrap();
    let (mut s, r) = task.await.unwrap();
    assert_eq!(r.unwrap_err().kind, ErrorKind::Cancelled);
    run(&mut s, "INSERT INTO t VALUES (1)").await;
    assert_eq!(
        count(&mut s, "t").await,
        "0",
        "still read-only on the new connection"
    );
}

#[tokio::test]
async fn an_actual_plan_does_not_leave_writes_in_read_only_mode() {
    let db = ms!();
    let mut s = session(&db).await;
    run(&mut s, "CREATE TABLE t (id int)").await;
    s.set_read_only(true).await.unwrap();
    s.explain("INSERT INTO t VALUES (1)", true).await.unwrap();
    assert_eq!(count(&mut s, "t").await, "0");
    // 調べる文自身も包まれるので、モードを戻してから数える
    s.set_read_only(false).await.unwrap();
    let (_, r) = run(&mut s, "SELECT @@TRANCOUNT").await;
    assert_eq!(r.text(0, 0).as_deref(), Some("0"));
}
