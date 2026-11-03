//! 実行計画(PostgreSQL の `EXPLAIN (FORMAT JSON)`)の木構造への変換。DB に依存しない純粋なロジック。
//!
//! 画面は、この木を、コストや時間の大きいノードが目立つように描く。

use serde_json::Value;

/// 計画の 1 ノード。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    /// `Seq Scan`、`Hash Join` など
    pub kind: String,
    /// 対象のテーブル、索引など(`on users`、`using users_pkey`)
    pub target: Option<String>,
    pub startup_cost: f64,
    pub total_cost: f64,
    /// 見積もりの行数
    pub rows: f64,
    /// `ANALYZE` した場合の、実際の時間(ミリ秒)と行数。1 回あたりの値に、実行回数を掛けたもの
    pub actual_ms: Option<f64>,
    pub actual_rows: Option<f64>,
    pub loops: f64,
    /// 条件(`Filter`、`Index Cond`、`Join Filter` など)
    pub conditions: Vec<(String, String)>,
    pub children: Vec<PlanNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub root: PlanNode,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
}

impl PlanNode {
    /// このノード自身の分(子のコストを除く)。重いノードを見つけるのに使う。
    pub fn self_cost(&self) -> f64 {
        let children: f64 = self.children.iter().map(|c| c.total_cost).sum();
        (self.total_cost - children).max(0.0)
    }

    /// このノード自身の時間(ミリ秒)。`ANALYZE` した計画だけ。
    pub fn self_ms(&self) -> Option<f64> {
        let total = self.actual_ms?;
        let children: f64 = self.children.iter().filter_map(|c| c.actual_ms).sum();
        Some((total - children).max(0.0))
    }

    /// 木の中で、自分以下の全ノード(自分を含む)。
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a PlanNode>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

impl Plan {
    /// 全ノードのうち、最も重いもの(`ANALYZE` なら時間、そうでなければコスト)。
    pub fn heaviest(&self) -> &PlanNode {
        let mut all = Vec::new();
        self.root.walk(&mut all);
        let weight = |n: &PlanNode| n.self_ms().unwrap_or_else(|| n.self_cost());
        all.into_iter()
            .max_by(|a, b| weight(a).total_cmp(&weight(b)))
            .unwrap_or(&self.root)
    }
}

/// `EXPLAIN (FORMAT JSON)` の出力(1 列 1 行の JSON 文字列)を解析する。
pub fn parse_postgres(json: &str) -> Result<Plan, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("実行計画の JSON を読めません: {e}"))?;
    let top = v
        .as_array()
        .and_then(|a| a.first())
        .ok_or("実行計画が空です")?;
    let plan = top.get("Plan").ok_or("実行計画に Plan がありません")?;
    Ok(Plan {
        root: node(plan)?,
        planning_ms: top.get("Planning Time").and_then(Value::as_f64),
        execution_ms: top.get("Execution Time").and_then(Value::as_f64),
    })
}

/// ノードの条件として見せるキー。
const CONDITION_KEYS: [&str; 9] = [
    "Filter",
    "Index Cond",
    "Recheck Cond",
    "Join Filter",
    "Hash Cond",
    "Merge Cond",
    "Sort Key",
    "Group Key",
    "One-Time Filter",
];

fn node(v: &Value) -> Result<PlanNode, String> {
    let num = |k: &str| v.get(k).and_then(Value::as_f64);
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = text("Node Type").ok_or("ノードの種類(Node Type)がありません")?;
    let target = match (text("Relation Name"), text("Index Name")) {
        (Some(rel), Some(idx)) => Some(format!("{rel} using {idx}")),
        (Some(rel), None) => Some(match text("Alias").filter(|a| *a != rel) {
            Some(alias) => format!("{rel} {alias}"),
            None => rel,
        }),
        (None, Some(idx)) => Some(idx),
        (None, None) => None,
    };
    let loops = num("Actual Loops").unwrap_or(1.0);
    let conditions = CONDITION_KEYS
        .iter()
        .filter_map(|&k| {
            let value = match v.get(k)? {
                Value::String(s) => s.clone(),
                Value::Array(a) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => return None,
            };
            Some((k.to_string(), value))
        })
        .collect();
    let children = v
        .get("Plans")
        .and_then(Value::as_array)
        .map(|plans| plans.iter().map(node).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();
    Ok(PlanNode {
        kind,
        target,
        startup_cost: num("Startup Cost").unwrap_or(0.0),
        total_cost: num("Total Cost").unwrap_or(0.0),
        rows: num("Plan Rows").unwrap_or(0.0),
        // 1 回あたりの時間と行数に、実行回数を掛けて、合計にする
        actual_ms: num("Actual Total Time").map(|t| t * loops),
        actual_rows: num("Actual Rows").map(|r| r * loops),
        loops,
        conditions,
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"[{
      "Plan": {
        "Node Type": "Hash Join", "Startup Cost": 1.5, "Total Cost": 30.0, "Plan Rows": 100,
        "Hash Cond": "(o.customer_id = c.id)",
        "Plans": [
          {"Node Type": "Seq Scan", "Relation Name": "orders", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 20.0, "Plan Rows": 1000,
           "Filter": "(total > 10)"},
          {"Node Type": "Index Scan", "Relation Name": "customers", "Alias": "customers", "Index Name": "customers_pkey",
           "Startup Cost": 0.1, "Total Cost": 4.0, "Plan Rows": 1, "Index Cond": "(id = o.customer_id)"}
        ]
      },
      "Planning Time": 0.25
    }]"#;

    #[test]
    fn parses_the_tree_with_targets_and_conditions() {
        let p = parse_postgres(PLAN).unwrap();
        assert_eq!(p.root.kind, "Hash Join");
        assert_eq!(p.root.children.len(), 2);
        assert_eq!(p.root.children[0].target.as_deref(), Some("orders o"));
        assert_eq!(
            p.root.children[1].target.as_deref(),
            Some("customers using customers_pkey")
        );
        assert_eq!(
            p.root.conditions,
            [("Hash Cond".into(), "(o.customer_id = c.id)".into())]
        );
        assert_eq!(p.root.children[0].conditions[0].0, "Filter");
        assert_eq!((p.planning_ms, p.execution_ms), (Some(0.25), None));
    }

    #[test]
    fn self_cost_excludes_the_children_and_the_heaviest_node_is_found() {
        let p = parse_postgres(PLAN).unwrap();
        assert_eq!(p.root.self_cost(), 6.0, "30 - (20 + 4)");
        assert_eq!(p.heaviest().kind, "Seq Scan");
    }

    #[test]
    fn analyze_times_are_multiplied_by_loops_and_self_time_excludes_children() {
        let json = r#"[{"Plan": {"Node Type": "Nested Loop", "Total Cost": 10, "Actual Total Time": 5.0, "Actual Rows": 3, "Actual Loops": 1,
            "Plans": [{"Node Type": "Index Scan", "Index Name": "i", "Total Cost": 2, "Actual Total Time": 0.5, "Actual Rows": 1, "Actual Loops": 4}]},
            "Planning Time": 0.1, "Execution Time": 5.2}]"#;
        let p = parse_postgres(json).unwrap();
        let child = &p.root.children[0];
        assert_eq!((child.actual_ms, child.actual_rows), (Some(2.0), Some(4.0)));
        assert_eq!(p.root.self_ms(), Some(3.0));
        assert_eq!(p.execution_ms, Some(5.2));
        assert_eq!(
            p.heaviest().kind,
            "Nested Loop",
            "3.0 ms of its own vs 2.0 ms"
        );
    }

    #[test]
    fn malformed_input_is_reported() {
        assert!(parse_postgres("not json").is_err());
        assert!(parse_postgres("[]").is_err());
        assert!(parse_postgres(r#"[{"Foo": 1}]"#).is_err());
        assert!(
            parse_postgres(r#"[{"Plan": {"Total Cost": 1}}]"#)
                .unwrap_err()
                .contains("Node Type")
        );
    }
}
