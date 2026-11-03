//! 100 万行 20 列を、実際の PostgreSQL から core 経由で受信して、時間とメモリを測る。
//! `cargo run --release -p sqltool-core --example bench_pg -- [メモリ上限MiB] [spill] [nosort]`
//! メモリは `/usr/bin/time -l`(macOS)や `-v`(Linux)の最大 RSS で見る。

use std::sync::Arc;
use std::time::Instant;

use driver_postgres::PostgresDriver;
use sqltool_core::*;
use storage::MemorySecretStore;

#[tokio::main]
async fn main() {
    let limit_mb: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(512);
    let spill = std::env::args().nth(2).as_deref() == Some("spill");
    let db = test_pg::create_database().expect("postgres");

    let drivers = DriverRegistry::new().with("postgresql", Arc::new(PostgresDriver));
    let mut cfg = CoreConfig::new(Arc::new(MemorySecretStore::new()), drivers);
    cfg.memory_limit = limit_mb << 20;
    let spill_dir = std::env::temp_dir().join("sqltool-bench-spill");
    cfg.spill_dir = spill.then(|| spill_dir.clone());
    let (core, mut events) = Core::start(cfg, tokio::runtime::Handle::current()).unwrap();

    core.send(Command::SaveConnection(ConnectionInput {
        id: None,
        name: "bench".into(),
        driver: "postgresql".into(),
        host: Some(db.host.clone()),
        port: Some(db.port),
        database: Some(db.dbname.clone()),
        username: Some(db.user.clone()),
        options: ConnectionOptions {
            tls: TlsSetting::Disable,
            ..Default::default()
        },
        env_label: EnvLabel::Dev,
        password: PasswordChange::Keep,
        ssh_secret: PasswordChange::Keep,
    }));
    let conn = loop {
        if let Some(Event::ConnectionSaved(c)) = events.recv().await {
            break c;
        }
    };
    core.send(Command::OpenTab {
        connection_id: conn.id,
    });
    let tab = loop {
        if let Some(Event::TabOpened { tab, .. }) = events.recv().await {
            break tab;
        }
    };

    // 20 列: 整数、小数、文字列、真偽、NULL を混ぜる
    let cols: Vec<String> = (0..20)
        .map(|i| match i % 5 {
            0 => format!("g + {i} AS c{i}"),
            1 => format!("(g::numeric / 7) AS c{i}"),
            2 => format!("'value-' || (g % 1000) AS c{i}"),
            3 => format!("(g % 2 = 0) AS c{i}"),
            _ => format!("CASE WHEN g % 10 = 0 THEN NULL ELSE 'x' || g END AS c{i}"),
        })
        .collect();
    let sql = format!(
        "SELECT {} FROM generate_series(1, 1000000) g",
        cols.join(", ")
    );

    let started = Instant::now();
    core.send(Command::Execute {
        tab,
        sql,
        target: ExecuteTarget::All,
        options: ExecuteOptions {
            fetch_limit: FetchLimit::Unlimited,
            ..Default::default()
        },
    });
    let mut handle = None;
    let mut events_seen = 0u64;
    let outcome = loop {
        match events.recv().await.unwrap() {
            Event::ResultOpened { result, .. } => handle = Some(result),
            Event::StatementFinished { outcome, .. } => break outcome,
            _ => {}
        }
        events_seen += 1;
    };
    let elapsed = started.elapsed();
    let handle = handle.unwrap();
    let stats = handle.stats();
    println!("outcome: {outcome:?}");
    println!("receive 1M x 20: {elapsed:?} ({events_seen} other events)");
    println!(
        "result store: resident {} MiB, spilled {} MiB",
        stats.resident_bytes >> 20,
        stats.spilled_bytes >> 20
    );

    let t = Instant::now();
    let mut ready = 0u64;
    for i in (0..handle.row_count()).step_by(997) {
        if let Access::Ready(()) = handle.with_row(i, |r| {
            let _ = r.cell(1);
        }) {
            ready += 1;
        }
    }
    println!("sampled {ready} rows via with_row in {:?}", t.elapsed());

    // 全件を受信できた結果だけが並べ替えられる(打ち切られた結果は、拒否される)
    let skip_sort = std::env::args().any(|a| a == "nosort");
    if handle.can_sort() && !skip_sort {
        let t = Instant::now();
        core.send(Command::SortResult {
            result: handle.id(),
            column: 1,
            order: SortOrder::Descending,
        });
        loop {
            if let Event::SortChanged { .. } = events.recv().await.unwrap() {
                break;
            }
        }
        println!("numeric sort (col 1): {:?}", t.elapsed());
    } else {
        println!("not sortable (truncated), as expected");
    }
    let _ = std::fs::remove_dir_all(spill_dir);
}
