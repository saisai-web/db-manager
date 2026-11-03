//! 行は届いた順にストアへ入り、画面は完了を待たずに先頭を読める(体感速度の前提)。

use std::time::{Duration, Instant};

use sqltool_core::*;

use crate::common::*;

/// 1 行ずつ 20ms かかる 150 行(約 3 秒)。1 行 1KB なので、サーバーの送信バッファも数行ごとに流れる。
const SLOW: &str =
    "select g, repeat('x', 1000) as pad, pg_sleep(0.02) from generate_series(1, 150) g";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_rows_are_readable_long_before_the_statement_finishes() {
    let db = pg!();
    let mut h = H::start(db).await;
    let tab = h.open_tab(EnvLabel::Dev).await;
    h.send_exec(tab, SLOW, ExecuteTarget::All, ExecuteOptions::default());

    let t0 = Instant::now();
    let evs = h.until(|e| matches!(e, Event::ResultOpened { .. })).await;
    let handle = result_handle(&evs);

    let mut first_rows = None;
    let mut notified_before_finish = 0;
    let mut finished = None;
    while finished.is_none() {
        tokio::select! {
            e = h.recv() => match e {
                Event::Progress { .. } => notified_before_finish += 1,
                Event::StatementFinished { .. } => finished = Some(t0.elapsed()),
                _ => {}
            },
            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
        if first_rows.is_none() && handle.row_count() > 0 {
            first_rows = Some((t0.elapsed(), handle.row_count()));
        }
    }
    let (first_at, n) = first_rows.expect("rows were stored");
    let finished_at = finished.unwrap();
    eprintln!(
        "first rows: {n} rows at {first_at:?}; finished at {finished_at:?}; progress events: {notified_before_finish}"
    );
    assert!(
        first_at * 4 < finished_at,
        "rows must be readable early ({first_at:?} vs {finished_at:?})"
    );
    assert!(
        notified_before_finish > 0,
        "the UI must be told rows arrived, or it cannot repaint before the end"
    );
}
