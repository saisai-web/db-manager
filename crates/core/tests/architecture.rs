//! 設計書「アーキテクチャ」のクレート構成(依存の向き)を守るためのテスト。
//!
//! ```text
//! app ──► core ──► driver-api ◄── driver-*
//!            ├────► sqlscan
//!            ├────► result-store
//!            ├────► storage
//!            └────► data-io
//! ```
//!
//! 実行時の依存だけを見る(テスト用の dev 依存は対象外)。依存を足したくなったら、
//! 設計を見直すか、設計書とこのテストを意図して更新する。

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// ワークスペース内のクレート名 → 実行時に依存するワークスペース内のクレート名
fn runtime_graph() -> BTreeMap<String, BTreeSet<String>> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata");
    assert!(out.status.success());
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let packages = meta["packages"].as_array().unwrap();
    let members: BTreeSet<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    packages
        .iter()
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["kind"].is_null()) // null = 通常の依存(dev / build ではない)
                .map(|d| d["name"].as_str().unwrap())
                .filter(|n| members.contains(n))
                .map(String::from)
                .collect();
            (p["name"].as_str().unwrap().to_string(), deps)
        })
        .collect()
}

fn deps(g: &BTreeMap<String, BTreeSet<String>>, name: &str) -> Vec<String> {
    g[name].iter().cloned().collect()
}

#[test]
fn leaf_crates_depend_on_no_other_workspace_crate() {
    let g = runtime_graph();
    for leaf in [
        "sqlscan",
        "result-store",
        "driver-api",
        "storage",
        "data-io",
    ] {
        assert!(
            deps(&g, leaf).is_empty(),
            "{leaf} must not depend on {:?}",
            deps(&g, leaf)
        );
    }
}

#[test]
fn drivers_depend_only_on_driver_api() {
    let g = runtime_graph();
    assert_eq!(deps(&g, "driver-postgres"), ["driver-api"]);
    assert_eq!(deps(&g, "driver-sqlite"), ["driver-api"]);
}

#[test]
fn the_ssh_tunnel_depends_only_on_driver_api() {
    let g = runtime_graph();
    assert_eq!(deps(&g, "ssh-tunnel"), ["driver-api"]);
}

#[test]
fn core_depends_only_on_the_lower_layers_and_never_on_a_concrete_driver() {
    let g = runtime_graph();
    assert_eq!(
        deps(&g, "sqltool-core"),
        [
            "data-io",
            "driver-api",
            "result-store",
            "sqlscan",
            "ssh-tunnel",
            "storage"
        ],
        "core must not know any concrete driver (no DB-name branching) or the UI"
    );
}

#[test]
fn no_library_crate_depends_on_core_or_the_ui() {
    let g = runtime_graph();
    for (name, ds) in &g {
        // 開発用の CLI(と、将来の app)だけが core を使える
        if name == "sqltool-cli" || name.starts_with("app") {
            continue;
        }
        assert!(
            !ds.contains("sqltool-core"),
            "{name} must not depend on core"
        );
    }
}

#[test]
fn test_support_is_not_a_runtime_dependency_of_the_product_layers() {
    let g = runtime_graph();
    for (name, ds) in &g {
        if name == "sqltool-cli" {
            continue; // 開発用 CLI だけは、使い捨ての DB のために使う
        }
        for support in ["test-pg", "test-ssh"] {
            assert!(
                !ds.contains(support),
                "{name} must not depend on {support} at runtime"
            );
        }
    }
}

/// UI(app)は `core` と、使うドライバにだけ依存する。下位の層を直接触らない。
#[test]
fn the_ui_layer_talks_to_core_only() {
    let g = runtime_graph();
    for (name, ds) in g.iter().filter(|(n, _)| n.starts_with("app")) {
        for forbidden in ["sqlscan", "result-store", "storage"] {
            assert!(
                !ds.contains(forbidden),
                "{name} must go through core, not {forbidden}"
            );
        }
    }
}

/// 1 つのソースファイルが肥大化しないための上限(行数)。超えたら、責務ごとにモジュールを分ける。
/// 上限を引き上げて済ませないこと。
const MAX_LINES: usize = 500;

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_file_exceeds_the_line_limit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let too_long: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let n = std::fs::read_to_string(f).ok()?.lines().count();
            (n > MAX_LINES)
                .then(|| format!("{} ({n} 行)", f.strip_prefix(&root).unwrap().display()))
        })
        .collect();
    assert!(
        too_long.is_empty(),
        "{MAX_LINES} 行を超えるファイルがあります。責務ごとに分割してください:\n  {}",
        too_long.join("\n  ")
    );
}
