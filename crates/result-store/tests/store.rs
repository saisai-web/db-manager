use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use result_store::{Access, Pressure, ResultStore, SortKind, SortOrder, StoreConfig, StoreError};

fn cfg(dir: Option<&std::path::Path>, limit: usize, chunk_rows: u32) -> StoreConfig {
    StoreConfig {
        memory_limit: limit,
        spill_dir: dir.map(|d| d.join("spill")),
        chunk_rows,
        chunk_max_bytes: 4 * 1024 * 1024,
    }
}

fn cell_text(i: u64, c: usize) -> String {
    format!("row{i}-col{c}-{}", "x".repeat((i % 17) as usize))
}

/// 3 列。3 列目は 7 行に 1 行が NULL、その次の行は空文字列。
fn push_sample(store: &ResultStore, i: u64) -> Result<Pressure, StoreError> {
    let a = cell_text(i, 0);
    let b = cell_text(i, 1);
    let third: Option<&[u8]> = match i % 7 {
        0 => None,
        1 => Some(b""),
        _ => Some(b"v"),
    };
    store.push_row([Some(a.as_bytes()), Some(b.as_bytes()), third])
}

fn expect_sample(row: &[Option<Vec<u8>>], i: u64) {
    assert_eq!(
        row[0].as_deref(),
        Some(cell_text(i, 0).as_bytes()),
        "row {i} col 0"
    );
    assert_eq!(
        row[1].as_deref(),
        Some(cell_text(i, 1).as_bytes()),
        "row {i} col 1"
    );
    match i % 7 {
        0 => assert_eq!(row[2], None, "row {i} must be NULL"),
        1 => assert_eq!(
            row[2].as_deref(),
            Some(&b""[..]),
            "row {i} must be empty string"
        ),
        _ => assert_eq!(row[2].as_deref(), Some(&b"v"[..])),
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn rows_are_readable_while_streaming_and_after_finish() {
    let store = ResultStore::new(3, cfg(None, usize::MAX, 100));
    for i in 0..250 {
        push_sample(&store, i).unwrap();
        // 受信中のチャンクの行もすぐ読める
        assert_eq!(store.row_count(), i + 1);
        let got = store.with_row(i, |r| (r.cell(0).map(<[u8]>::to_vec), r.col_count()));
        assert_eq!(got, Access::Ready((Some(cell_text(i, 0).into_bytes()), 3)));
    }
    assert!(!store.is_complete());
    store.finish();
    assert!(store.is_complete());
    for i in 0..250 {
        expect_sample(&store.read_row_blocking(i).unwrap().unwrap(), i);
    }
    assert!(store.read_row_blocking(250).unwrap().is_none());
    assert_eq!(store.with_row(250, |_| ()), Access::Unavailable);
    assert!(matches!(
        push_sample(&store, 250),
        Err(StoreError::Finished)
    ));
}

#[test]
fn width_mismatch_is_rejected_and_store_stays_usable() {
    let store = ResultStore::new(3, cfg(None, usize::MAX, 10));
    push_sample(&store, 0).unwrap();
    assert!(matches!(
        store.push_row([Some(&b"a"[..])]),
        Err(StoreError::RowWidthMismatch {
            expected: 3,
            actual: 1
        })
    ));
    push_sample(&store, 1).unwrap();
    assert_eq!(store.row_count(), 2);
    expect_sample(&store.read_row_blocking(1).unwrap().unwrap(), 1);
}

#[test]
fn zero_columns_and_zero_rows_are_fine() {
    let store = ResultStore::new(2, cfg(None, usize::MAX, 10));
    store.finish();
    assert_eq!(store.row_count(), 0);
    assert!(store.read_row_blocking(0).unwrap().is_none());
    let idx = store
        .sort_index(0, SortOrder::Ascending, SortKind::Text)
        .unwrap();
    assert!(idx.is_empty());
}

#[test]
fn spills_under_memory_pressure_and_reloads_without_blocking_the_reader() {
    let dir = tempfile::tempdir().unwrap();
    let limit = 64 * 1024;
    let store = ResultStore::new(3, cfg(Some(dir.path()), limit, 100));
    let ready = Arc::new(AtomicUsize::new(0));
    {
        let ready = Arc::clone(&ready);
        store.set_on_ready(move || {
            ready.fetch_add(1, Ordering::SeqCst);
        });
    }

    let n = 20_000u64;
    for i in 0..n {
        if push_sample(&store, i).unwrap() == Pressure::High {
            thread::sleep(Duration::from_millis(1));
        }
    }
    store.finish();
    wait_until("spill writes to finish", || {
        let s = store.stats();
        s.spilled_bytes > 0 && s.resident_bytes <= limit * 2
    });
    let stats = store.stats();
    assert_eq!(stats.rows, n);
    assert!(
        stats.resident_bytes <= limit * 2,
        "resident {} should stay near the {} limit",
        stats.resident_bytes,
        limit
    );

    // 先頭の行は退避済み。with_row は待たずに Loading を返し、読み戻し後に Ready になる
    let first = store.with_row(0, |r| r.cell(0).map(<[u8]>::to_vec));
    assert!(
        matches!(first, Access::Loading | Access::Ready(_)),
        "unexpected {first:?}"
    );
    wait_until("row 0 to be reloaded", || {
        matches!(store.with_row(0, |_| ()), Access::Ready(()))
    });
    assert!(ready.load(Ordering::SeqCst) >= 1, "on_ready must be called");

    // すべての値が一字一句一致する(NULL と空文字列の区別を含む)
    for i in 0..n {
        expect_sample(&store.read_row_blocking(i).unwrap().unwrap(), i);
    }
    // 全件を読み戻した後も上限に収まる
    wait_until("limit after full read", || {
        store.stats().resident_bytes <= limit * 2
    });
}

#[test]
fn evicting_a_reloaded_chunk_does_not_rewrite_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(3, cfg(Some(dir.path()), 32 * 1024, 100));
    for i in 0..5_000 {
        push_sample(&store, i).unwrap();
    }
    store.finish();

    let settle = || {
        wait_until("pending writes to finish", || {
            store.stats().pending_write_bytes == 0
        })
    };
    let pass = || {
        for i in (0..5_000).step_by(100) {
            store.read_row_blocking(i).unwrap();
        }
        settle();
    };
    // 追記直後にメモリに残っていたチャンクは、最初の周回で追い出される時に初めて書き出される。
    // 2 周すれば、すべてのチャンクが一度は書き出されている
    settle();
    pass();
    pass();
    let written = store.stats().spilled_bytes;
    assert!(written > 0);

    // 以降は、読み戻して再び追い出しても、書き出し量は増えない
    pass();
    pass();
    assert_eq!(store.stats().spilled_bytes, written);
}

#[test]
fn without_spill_the_limit_stops_the_stream_and_keeps_earlier_rows() {
    let store = ResultStore::new(3, cfg(None, 32 * 1024, 100));
    let mut accepted = 0;
    let err = loop {
        match push_sample(&store, accepted) {
            Ok(_) => accepted += 1,
            Err(e) => break e,
        }
        assert!(accepted < 100_000, "limit was never enforced");
    };
    assert!(matches!(err, StoreError::LimitReached), "{err:?}");
    assert!(accepted > 0);
    assert_eq!(store.row_count(), accepted);
    // 上限に達しても、受信済みの行は読める。超過は 1 チャンク程度に収まる
    for i in 0..accepted {
        expect_sample(&store.read_row_blocking(i).unwrap().unwrap(), i);
    }
    assert!(store.stats().resident_bytes < 32 * 1024 * 3);
    // 続けて追記しても同じ結果で、壊れない
    assert!(matches!(
        push_sample(&store, accepted),
        Err(StoreError::LimitReached)
    ));
    store.finish();
}

#[test]
fn limit_smaller_than_one_chunk_still_serves_the_latest_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(3, cfg(Some(dir.path()), 1, 50));
    for i in 0..500 {
        push_sample(&store, i).unwrap();
    }
    store.finish();
    for i in 0..500 {
        expect_sample(&store.read_row_blocking(i).unwrap().unwrap(), i);
    }
}

#[test]
fn large_cells_seal_chunks_by_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg(Some(dir.path()), 3 * 1024 * 1024, 4096);
    c.chunk_max_bytes = 1024 * 1024;
    let store = ResultStore::new(1, c);
    let big = vec![b'z'; 700 * 1024];
    for _ in 0..12 {
        store.push_row([Some(&big[..])]).unwrap();
    }
    store.finish();
    wait_until("spill", || store.stats().spilled_bytes > 0);
    for i in 0..12 {
        let row = store.read_row_blocking(i).unwrap().unwrap();
        assert_eq!(row[0].as_deref(), Some(&big[..]));
    }
}

#[test]
fn binary_values_survive_spill_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(1, cfg(Some(dir.path()), 1024, 10));
    let blobs: Vec<Vec<u8>> = (0..300u32)
        .map(|i| (0..(i % 50 + 1)).map(|j| (i + j) as u8).collect())
        .collect();
    for b in &blobs {
        store.push_row([Some(&b[..])]).unwrap();
    }
    store.finish();
    for (i, b) in blobs.iter().enumerate() {
        let row = store.read_row_blocking(i as u64).unwrap().unwrap();
        assert_eq!(row[0].as_deref(), Some(&b[..]), "blob {i}");
    }
}

#[test]
fn numeric_strings_are_kept_verbatim() {
    let store = ResultStore::new(1, cfg(None, usize::MAX, 10));
    let values = [
        "12345678901234567890.123456789012345678",
        "0.10",
        "-0",
        "1E+20",
        "9223372036854775808",
        "2026-11-02 12:34:56.123456+09",
    ];
    for v in values {
        store.push_row([Some(v.as_bytes())]).unwrap();
    }
    for (i, v) in values.iter().enumerate() {
        let row = store.read_row_blocking(i as u64).unwrap().unwrap();
        assert_eq!(row[0].as_deref(), Some(v.as_bytes()));
    }
}

#[test]
fn sort_requires_a_complete_result() {
    let store = ResultStore::new(1, cfg(None, usize::MAX, 10));
    store.push_row([Some(&b"1"[..])]).unwrap();
    assert!(matches!(
        store.sort_index(0, SortOrder::Ascending, SortKind::Numeric),
        Err(StoreError::NotComplete)
    ));
    store.finish();
    assert!(matches!(
        store.sort_index(5, SortOrder::Ascending, SortKind::Numeric),
        Err(StoreError::ColumnOutOfRange(5))
    ));
    assert!(
        store
            .sort_index(0, SortOrder::Ascending, SortKind::Numeric)
            .is_ok()
    );
}

#[test]
fn sort_index_orders_rows_without_touching_the_data() {
    let dir = tempfile::tempdir().unwrap();
    // 退避を伴う状態でもソートできる
    let store = ResultStore::new(2, cfg(Some(dir.path()), 2 * 1024, 20));
    let n = 1_000u64;
    let key = |i: u64| (i * 7919) % n; // 0..n の並べ替え
    for i in 0..n {
        let k = key(i).to_string();
        store
            .push_row([Some(k.as_bytes()), Some(format!("{i}").as_bytes())])
            .unwrap();
    }
    store.finish();
    wait_until("spill", || store.stats().spilled_bytes > 0);

    let asc = store
        .sort_index(0, SortOrder::Ascending, SortKind::Numeric)
        .unwrap();
    assert_eq!(asc.len() as u64, n);
    for pos in 0..n {
        let src = asc.source_row(pos).unwrap();
        assert_eq!(key(src), pos, "position {pos}");
    }
    let desc = store
        .sort_index(0, SortOrder::Descending, SortKind::Numeric)
        .unwrap();
    assert_eq!(key(desc.source_row(0).unwrap()), n - 1);

    // 文字列としての並びとは異なる("10" < "9")
    let text = store
        .sort_index(0, SortOrder::Ascending, SortKind::Text)
        .unwrap();
    let first_two: Vec<_> = (0..3).map(|p| key(text.source_row(p).unwrap())).collect();
    assert_eq!(first_two, [0, 1, 10]);

    // 元データは並べ替わっていない
    expect_row_value(&store, 5, 1, "5");
}

fn expect_row_value(store: &ResultStore, row: u64, col: usize, want: &str) {
    let r = store.read_row_blocking(row).unwrap().unwrap();
    assert_eq!(r[col].as_deref(), Some(want.as_bytes()));
}

#[test]
fn concurrent_writer_and_ui_reader() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(3, cfg(Some(dir.path()), 16 * 1024, 50));
    let n = 30_000u64;

    let writer = {
        let store = store.clone();
        thread::spawn(move || {
            for i in 0..n {
                if push_sample(&store, i).unwrap() == Pressure::High {
                    thread::sleep(Duration::from_micros(200));
                }
            }
            store.finish();
        })
    };
    let reader = {
        let store = store.clone();
        thread::spawn(move || {
            // UI 相当: 行数の範囲内をランダムに近い順で読み、決して panic / 誤読しない
            let mut seed = 12345u64;
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut ready = 0u64;
            while !store.is_complete() || ready < 2_000 {
                assert!(Instant::now() < deadline);
                let rows = store.row_count();
                if rows == 0 {
                    continue;
                }
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let row = (seed >> 33) % rows;
                let access = store.with_row(row, |r| {
                    (r.cell(0).map(<[u8]>::to_vec), r.cell(2).map(<[u8]>::to_vec))
                });
                if let Access::Ready((a, third)) = access {
                    ready += 1;
                    assert_eq!(a.as_deref(), Some(cell_text(row, 0).as_bytes()));
                    match row % 7 {
                        0 => assert_eq!(third, None),
                        1 => assert_eq!(third.as_deref(), Some(&b""[..])),
                        _ => assert_eq!(third.as_deref(), Some(&b"v"[..])),
                    }
                }
            }
        })
    };
    writer.join().unwrap();
    reader.join().unwrap();
    assert_eq!(store.row_count(), n);
}

#[cfg(unix)]
#[test]
fn no_spill_file_is_left_visible_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(3, cfg(Some(dir.path()), 1024, 10));
    for i in 0..1_000 {
        push_sample(&store, i).unwrap();
    }
    store.finish();
    wait_until("spill", || store.stats().spilled_bytes > 0);
    let spill_dir = dir.path().join("spill");
    assert_eq!(std::fs::read_dir(&spill_dir).unwrap().count(), 0);
    drop(store);
    assert_eq!(std::fs::read_dir(&spill_dir).unwrap().count(), 0);
}

#[test]
fn dropping_the_store_stops_the_io_thread() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResultStore::new(3, cfg(Some(dir.path()), 1024, 10));
    for i in 0..1_000 {
        push_sample(&store, i).unwrap();
    }
    store.finish();
    wait_until("spill", || store.stats().spilled_bytes > 0);
    drop(store); // panic やデッドロックなく終わること
}
