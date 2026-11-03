//! 100 万行 20 列の追記・読み出し・ソートの所要時間とメモリを測る。
//! `cargo run --release -p result-store --example bench [memory_limit_mb]`

use std::time::Instant;

use result_store::{Access, ResultStore, SortKind, SortOrder, StoreConfig};

fn main() {
    let limit_mb: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(512);
    let dir = std::env::temp_dir().join("result-store-bench");
    let mut cfg = StoreConfig::new(Some(dir.clone()));
    cfg.memory_limit = limit_mb * 1024 * 1024;
    let (rows, cols) = (1_000_000u64, 20usize);
    let store = ResultStore::new(cols, cfg);

    let t = Instant::now();
    let mut line = vec![String::new(); cols];
    for i in 0..rows {
        for (c, cell) in line.iter_mut().enumerate() {
            cell.clear();
            use std::fmt::Write;
            match c {
                0 => write!(cell, "{i}").unwrap(),
                1 => write!(cell, "{}.{:04}", i * 31 % 100_000, i % 10_000).unwrap(),
                _ => write!(cell, "value-{}-{c}", i % 1000).unwrap(),
            }
        }
        store
            .push_row(
                line.iter()
                    .enumerate()
                    .map(|(c, s)| (c != 19 || i % 5 != 0).then_some(s.as_bytes())),
            )
            .unwrap();
    }
    store.finish();
    let s = store.stats();
    println!("append {rows} x {cols}: {:?}", t.elapsed());
    println!(
        "  resident {} MiB, spilled {} MiB",
        s.resident_bytes >> 20,
        s.spilled_bytes >> 20
    );

    let t = Instant::now();
    let mut sum = 0usize;
    for i in (0..rows).step_by(7) {
        if let Access::Ready(n) = store.with_row(i, |r| r.cell(0).map_or(0, <[u8]>::len)) {
            sum += n;
        }
    }
    println!("with_row sample scan: {:?} (checksum {sum})", t.elapsed());

    let t = Instant::now();
    let idx = store
        .sort_index(1, SortOrder::Descending, SortKind::Numeric)
        .unwrap();
    println!("sort numeric col: {:?}, {} rows", t.elapsed(), idx.len());
    let s = store.stats();
    println!(
        "  resident {} MiB, spilled {} MiB",
        s.resident_bytes >> 20,
        s.spilled_bytes >> 20
    );
    let _ = std::fs::remove_dir_all(dir);
}
