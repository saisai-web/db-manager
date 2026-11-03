//! core を動かして確かめるための開発用 CLI。製品の UI ではない。
//!
//! `cargo run -p sqltool-cli -- --demo`        使い捨ての PostgreSQL とサンプルデータで起動する
//! `cargo run -p sqltool-cli -- --host H --port P --db D --user U [--production]`
//!
//! 1 行 = 1 回の実行(全文)。`:q` 終了、`:ro on|off` 読み取り専用、`:schema` スキーマ一覧。
//! 実行中に Ctrl-C でキャンセルする。

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use driver_postgres::PostgresDriver;
use sqltool_core::*;
use storage::MemorySecretStore;
use tokio::io::{AsyncBufReadExt, BufReader};

const SEED: &str = "
create table customers (id bigint primary key, name text not null, email text, balance numeric(20,6));
insert into customers values
 (1, '佐伯 光平', 'saeki@example.com', 12345678901234.123456),
 (2, 'Alice', null, 0.5),
 (3, '', 'empty-name@example.com', -99.000001),
 (4, '山田 花子', 'hanako@example.com', 100);
create table orders (id serial primary key, customer_id bigint references customers(id), placed timestamptz default now(), total numeric(12,2));
insert into orders (customer_id, total) select (g % 4) + 1, (g * 13.37)::numeric(12,2) from generate_series(1, 30) g;
create view v_big_orders as select * from orders where total > 300;
";

fn width(s: &str) -> usize {
    s.chars()
        .map(|c| if c as u32 >= 0x2E80 { 2 } else { 1 })
        .sum()
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

fn show(h: &ResultHandle, max: u64) {
    let cols: Vec<&str> = h.columns().iter().map(|c| c.name.as_str()).collect();
    let n = h.row_count().min(max);
    let mut data: Vec<Vec<String>> = Vec::new();
    for i in 0..n {
        if let Access::Ready(row) = h.with_row(i, |r| {
            (0..cols.len())
                .map(|c| match r.cell(c) {
                    None => "∅(NULL)".to_string(),
                    Some([]) => "''".to_string(),
                    Some(b) if h.columns()[c].class == ValueClass::Binary => {
                        format!(
                            "0x{}…",
                            b.iter()
                                .take(8)
                                .map(|x| format!("{x:02x}"))
                                .collect::<String>()
                        )
                    }
                    Some(b) => String::from_utf8_lossy(b).into_owned(),
                })
                .collect::<Vec<_>>()
        }) {
            data.push(row);
        }
    }
    let widths: Vec<usize> = (0..cols.len())
        .map(|c| {
            data.iter()
                .map(|r| width(&r[c]))
                .chain([width(cols[c])])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line: String = widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┼");
    println!(
        "{}",
        cols.iter()
            .zip(&widths)
            .map(|(c, w)| format!(" {} ", pad(c, *w)))
            .collect::<Vec<_>>()
            .join("│")
    );
    println!("{line}");
    for r in &data {
        println!(
            "{}",
            r.iter()
                .zip(&widths)
                .map(|(c, w)| format!(" {} ", pad(c, *w)))
                .collect::<Vec<_>>()
                .join("│")
        );
    }
    if h.row_count() > n {
        println!("… 他 {} 行(受信済み)", h.row_count() - n);
    }
}

async fn ask(prompt: &str, lines: &mut tokio::io::Lines<BufReader<tokio::io::Stdin>>) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    lines
        .next_line()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |n: &str| {
        args.iter()
            .position(|a| a == n)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let demo = args.iter().any(|a| a == "--demo");

    let (host, port, db, user) = if demo {
        println!("使い捨ての PostgreSQL を起動しています…");
        let d = test_pg::create_database().expect("PostgreSQL を用意できません");
        (d.host, d.port, d.dbname, d.user)
    } else {
        (
            flag("--host").unwrap_or("localhost".into()),
            flag("--port").and_then(|p| p.parse().ok()).unwrap_or(5432),
            flag("--db").unwrap_or("postgres".into()),
            flag("--user").unwrap_or("postgres".into()),
        )
    };
    let production = args.iter().any(|a| a == "--production");

    let drivers = DriverRegistry::new().with("postgresql", Arc::new(PostgresDriver));
    let (core, mut ev) = Core::start(
        CoreConfig::new(Arc::new(MemorySecretStore::new()), drivers),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    core.send(Command::SaveConnection(ConnectionInput {
        id: None,
        name: format!("{user}@{host}:{port}/{db}"),
        driver: "postgresql".into(),
        host: Some(host),
        port: Some(port),
        database: Some(db),
        username: Some(user),
        options: ConnectionOptions {
            tls: TlsSetting::Disable,
            ..Default::default()
        },
        env_label: if production {
            EnvLabel::Production
        } else {
            EnvLabel::Dev
        },
        password: PasswordChange::Keep,
        ssh_secret: PasswordChange::Keep,
    }));
    let conn = loop {
        match ev.recv().await.unwrap() {
            Event::ConnectionSaved(c) => break c,
            Event::CommandFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    };
    core.send(Command::OpenTab {
        connection_id: conn.id.clone(),
    });
    let tab = loop {
        match ev.recv().await.unwrap() {
            Event::TabOpened {
                tab,
                read_only,
                server_version,
                ..
            } => {
                println!(
                    "接続しました: {}",
                    server_version.split(',').next().unwrap_or("")
                );
                if production {
                    println!(
                        "\x1b[41;97m 本番ラベル \x1b[0m 接続直後は読み取り専用です(:ro off で解除)"
                    );
                    let _ = read_only;
                }
                break tab;
            }
            Event::CommandFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    if demo {
        core.send(Command::Execute {
            tab,
            sql: SEED.into(),
            target: ExecuteTarget::All,
            options: Default::default(),
        });
        while !matches!(
            ev.recv().await,
            Some(Event::ExecutionFinished { .. }) | None
        ) {}
        println!(
            "サンプルデータ: customers / orders / v_big_orders\n例: select * from customers;  select count(*), sum(total) from orders;"
        );
    }

    let mut txn = TxnState::Idle;
    loop {
        let mark = match txn {
            TxnState::Idle => "",
            TxnState::Open => " [未コミット]",
            TxnState::Failed => " [中断:ROLLBACK が必要]",
        };
        print!("\nsql{mark}> ");
        let _ = std::io::stdout().flush();
        let Some(line) = lines.next_line().await.ok().flatten() else {
            break;
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        match line.as_str() {
            ":q" => break,
            ":ro on" | ":ro off" => {
                core.send(Command::SetReadOnly {
                    tab,
                    read_only: line.ends_with("on"),
                });
            }
            ":schema" => {
                core.send(Command::LoadSchemas {
                    connection_id: conn.id.clone(),
                });
            }
            _ => core.send(Command::Execute {
                tab,
                sql: line.clone(),
                target: ExecuteTarget::All,
                options: ExecuteOptions {
                    fetch_limit: FetchLimit::Rows(1000),
                    ..Default::default()
                },
            }),
        }
        let mut running = !line.starts_with(':');
        loop {
            let e = tokio::select! {
                e = ev.recv() => e,
                _ = tokio::signal::ctrl_c(), if running => { core.send(Command::Cancel { tab }); continue }
            };
            let Some(e) = e else { return };
            match e {
                Event::ConfirmationRequired { id, items, .. } => {
                    for i in &items {
                        println!("\x1b[33m⚠ 確認: {:?}: {}\x1b[0m", i.kind, i.summary);
                    }
                    let a = ask("実行しますか? [y/N] ", &mut lines).await;
                    core.send(Command::Confirm {
                        id,
                        accept: a == "y",
                    });
                }
                Event::ResultOpened { .. } => {}
                Event::FetchPaused { result, rows } => {
                    println!("({rows} 行で一時停止)");
                    if let Some(h) = core.result(result) {
                        show(&h, 15);
                    }
                    let a = ask(
                        "続きを取得 [n]次の1000行 / [a]すべて / [s]打ち切り: ",
                        &mut lines,
                    )
                    .await;
                    core.send(match a.as_str() {
                        "n" => Command::FetchMore {
                            result,
                            more: FetchMore::NextBatch,
                        },
                        "a" => Command::FetchMore {
                            result,
                            more: FetchMore::Rest,
                        },
                        _ => Command::StopFetch { result },
                    });
                }
                Event::StatementFinished {
                    result, outcome, ..
                } => {
                    if let Some(h) = result.and_then(|r| core.result(r)) {
                        show(&h, 15);
                    }
                    match outcome {
                        StatementOutcome::Completed {
                            rows,
                            affected_rows,
                            elapsed,
                            has_result_set,
                        } => {
                            if has_result_set {
                                println!("\x1b[32m✓ {rows} 行 ({elapsed:.0?})\x1b[0m");
                            } else {
                                println!(
                                    "\x1b[32m✓ 完了 影響行数 {} ({elapsed:.0?})\x1b[0m",
                                    affected_rows.unwrap_or(0)
                                );
                            }
                        }
                        StatementOutcome::Truncated { rows, reason, .. } => println!(
                            "\x1b[33m■ {rows} 行で打ち切り({reason:?})。並べ替え不可\x1b[0m"
                        ),
                        StatementOutcome::Failed {
                            error,
                            editor_offset,
                            ..
                        } => {
                            println!(
                                "\x1b[31m✗ {error}{}\x1b[0m",
                                error
                                    .code
                                    .as_ref()
                                    .map(|c| format!(" [{c}]"))
                                    .unwrap_or_default()
                            );
                            if let Some(o) = editor_offset {
                                println!("  エラー位置: 先頭から {o} バイト目");
                            }
                        }
                        StatementOutcome::Cancelled { .. } => {
                            println!("\x1b[33m■ キャンセルしました\x1b[0m")
                        }
                        StatementOutcome::TimedOut { .. } => println!("タイムアウト"),
                    }
                }
                Event::ExecutionFinished { status, .. } => {
                    if let ExecutionStatus::RefusedReadOnly { editor_offset, .. } = status {
                        println!(
                            "\x1b[31m✗ 読み取り専用モードのため実行しませんでした(位置 {editor_offset})\x1b[0m"
                        );
                    }
                    if status == ExecutionStatus::Declined {
                        println!("実行を取りやめました");
                    }
                    break;
                }
                Event::TransactionChanged { state, .. } => txn = state,
                Event::ReadOnlyChanged { read_only, .. } => {
                    println!("読み取り専用: {}", if read_only { "ON" } else { "OFF" });
                    break;
                }
                Event::Schemas { schemas, .. } => {
                    println!(
                        "{}",
                        schemas
                            .iter()
                            .filter(|s| !s.system)
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    break;
                }
                Event::CommandFailed { error, .. } => {
                    println!("\x1b[31m✗ {error}\x1b[0m");
                    break;
                }
                Event::RunStateChanged {
                    state: RunState::Idle,
                    ..
                } => running = false,
                _ => {}
            }
        }
    }
    core.send(Command::Shutdown);
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(ev.recv().await, Some(Event::ShutdownComplete) | None) {}
    })
    .await;
}
