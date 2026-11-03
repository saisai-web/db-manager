//! Oracle の実行計画(ドライバが計画表の行を JSON の配列にしたもの)の、木構造への変換。
//! DB に依存しない純粋なロジック。
//!
//! 各行は `id`、`parent_id`、`operation`、`options`、`object_name`、`cost`、`cardinality`、
//! `access_predicates`、`filter_predicates`。実測つきなら `starts`(実行回数)、`output_rows`、
//! `elapsed_us`(子を含む累積の時間、マイクロ秒)が付く。

use std::collections::HashMap;

use serde_json::Value;

use crate::explain::{Plan, PlanNode};

pub fn parse_oracle_json(json: &str) -> Result<Plan, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("実行計画の JSON を読めません: {e}"))?;
    let rows = v
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("実行計画が空です")?;

    let mut children: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut root = None;
    for (i, row) in rows.iter().enumerate() {
        match row.get("parent_id").and_then(Value::as_f64) {
            Some(p) => children.entry(p as i64).or_default().push(i),
            None => root = root.or(Some(i)),
        }
    }
    let root = root.ok_or("実行計画に根がありません")?;
    let root = build(rows, &children, root, 0)?;
    Ok(Plan {
        planning_ms: None,
        execution_ms: root.actual_ms,
        root,
    })
}

/// 計画の深さの上限(壊れた入力で、再帰が止まらなくなるのを防ぐ)
const MAX_DEPTH: usize = 512;

fn build(
    rows: &[Value],
    children: &HashMap<i64, Vec<usize>>,
    index: usize,
    depth: usize,
) -> Result<PlanNode, String> {
    if depth > MAX_DEPTH {
        return Err("実行計画が深すぎます".into());
    }
    let row = &rows[index];
    let num = |key: &str| row.get(key).and_then(Value::as_f64);
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let id = num("id").ok_or("実行計画の行に id がありません")? as i64;
    let kind = [text("operation"), text("options")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let conditions = [
        ("Access", text("access_predicates")),
        ("Filter", text("filter_predicates")),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|v| (k.to_string(), v.to_string())))
    .collect();
    let kids = children
        .get(&id)
        .map(|v| {
            v.iter()
                .map(|&i| build(rows, children, i, depth + 1))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(PlanNode {
        kind,
        target: text("object_name").map(str::to_string),
        startup_cost: 0.0,
        total_cost: num("cost").unwrap_or(0.0),
        rows: num("cardinality").unwrap_or(0.0),
        actual_ms: num("elapsed_us").map(|us| us / 1000.0),
        actual_rows: num("output_rows"),
        loops: num("starts").unwrap_or(1.0),
        conditions,
        children: kids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_a_tree_by_parent_id() {
        let plan = parse_oracle_json(
            r#"[
              {"id":0,"parent_id":null,"operation":"SELECT STATEMENT","options":null,"cost":5,"cardinality":3},
              {"id":1,"parent_id":0,"operation":"HASH JOIN","options":null,"cost":5,"cardinality":3,
               "access_predicates":"\"A\".\"ID\"=\"B\".\"ID\""},
              {"id":2,"parent_id":1,"operation":"TABLE ACCESS","options":"FULL","object_name":"A","cost":2,"cardinality":3},
              {"id":3,"parent_id":1,"operation":"TABLE ACCESS","options":"FULL","object_name":"B","cost":2,"cardinality":9,
               "filter_predicates":"\"V\"='b'"}
            ]"#,
        )
        .unwrap();
        assert_eq!(plan.root.kind, "SELECT STATEMENT");
        let join = &plan.root.children[0];
        assert_eq!(join.kind, "HASH JOIN");
        assert_eq!(join.conditions[0].0, "Access");
        assert_eq!(join.children.len(), 2);
        assert_eq!(join.children[0].kind, "TABLE ACCESS FULL");
        assert_eq!(join.children[0].target.as_deref(), Some("A"));
        assert_eq!(
            join.children[1].conditions,
            [("Filter".into(), "\"V\"='b'".into())]
        );
        assert_eq!(join.children[1].rows, 9.0);
        assert_eq!(plan.execution_ms, None, "no timings without a run");
        assert_eq!(plan.root.actual_ms, None);
    }

    #[test]
    fn a_measured_plan_has_times_rows_and_starts() {
        let plan = parse_oracle_json(
            r#"[
              {"id":0,"parent_id":null,"operation":"SELECT STATEMENT","cost":2,"cardinality":1,
               "starts":1,"output_rows":10,"elapsed_us":4000},
              {"id":1,"parent_id":0,"operation":"TABLE ACCESS","options":"FULL","object_name":"T","cost":2,
               "cardinality":1,"starts":1,"output_rows":10,"elapsed_us":1500}
            ]"#,
        )
        .unwrap();
        assert_eq!(plan.execution_ms, Some(4.0));
        let scan = &plan.root.children[0];
        assert_eq!(scan.actual_ms, Some(1.5));
        assert_eq!(scan.actual_rows, Some(10.0));
        assert_eq!(scan.loops, 1.0);
        // 自分の分は、子を除いた時間
        assert_eq!(plan.root.self_ms(), Some(2.5));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(parse_oracle_json("not json").is_err());
        assert!(parse_oracle_json("[]").is_err());
        assert!(parse_oracle_json(r#"[{"id":1,"parent_id":0}]"#).is_err());
        assert!(parse_oracle_json(r#"[{"parent_id":null}]"#).is_err());
    }
}
