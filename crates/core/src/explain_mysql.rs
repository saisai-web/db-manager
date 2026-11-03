//! MySQL(`EXPLAIN FORMAT=TREE` / `EXPLAIN ANALYZE`)と MariaDB(`FORMAT=JSON`)の実行計画の、木構造への変換。
//! DB に依存しない純粋なロジック。

use serde_json::Value;

use crate::explain::{Plan, PlanNode};

/// MySQL の木。1 行が 1 ノードで、深さは 4 文字のインデント。
/// `-> Nested loop inner join  (cost=0.9 rows=1) (actual time=0.015..0.018 rows=2 loops=1)`
pub fn parse_mysql_tree(text: &str) -> Result<Plan, String> {
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut roots: Vec<PlanNode> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let indent = line.len() - line.trim_start().len();
        let body = line
            .trim_start()
            .strip_prefix("-> ")
            .ok_or_else(|| format!("実行計画の行を読めません: {line}"))?;
        let depth = indent / 4;
        // 自分より深い(または同じ)ノードは、完成した子として親に付ける
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            close_top(&mut stack, &mut roots);
        }
        stack.push((depth, tree_node(body)));
    }
    while !stack.is_empty() {
        close_top(&mut stack, &mut roots);
    }
    let root = roots.into_iter().next().ok_or("実行計画が空です")?;
    Ok(Plan {
        planning_ms: None,
        execution_ms: root.actual_ms,
        root,
    })
}

fn close_top(stack: &mut Vec<(usize, PlanNode)>, roots: &mut Vec<PlanNode>) {
    let Some((_, node)) = stack.pop() else { return };
    match stack.last_mut() {
        Some((_, parent)) => parent.children.push(node),
        None => roots.push(node),
    }
}

/// 末尾の `(...)` を、見積もり・実測として取り出し、残りを説明にする。
fn tree_node(body: &str) -> PlanNode {
    let mut desc = body.trim_end().to_string();
    let (mut cost, mut rows) = (0.0, 0.0);
    let (mut actual_ms, mut actual_rows, mut loops) = (None, None, 1.0);
    while let Some((head, group)) = split_last_group(&desc) {
        if let Some(rest) = group.strip_prefix("cost=") {
            let mut it = rest.split_whitespace();
            // `cost=0.9` と `cost=0.9..1.2`(後者は終わりの値を使う)
            cost = it
                .next()
                .and_then(|c| c.rsplit("..").next())
                .and_then(|c| c.parse().ok())
                .unwrap_or(0.0);
            rows = field(&mut it, "rows=").unwrap_or(0.0);
        } else if let Some(rest) = group.strip_prefix("actual time=") {
            let mut it = rest.split_whitespace();
            let end = it
                .next()
                .and_then(|t| t.rsplit("..").next())
                .and_then(|t| t.parse::<f64>().ok());
            let r = field(&mut it, "rows=");
            loops = field(&mut it, "loops=").unwrap_or(1.0);
            // 1 回あたりの値に、実行回数を掛けて、合計にする(PostgreSQL の計画と同じ扱い)
            actual_ms = end.map(|t| t * loops);
            actual_rows = r.map(|r| r * loops);
        } else if group == "never executed" {
            (actual_ms, actual_rows, loops) = (Some(0.0), Some(0.0), 0.0);
        } else {
            break; // 説明の一部の括弧(`(id = o.cid)` など)
        }
        desc = head.to_string();
    }
    let (kind, target, conditions) = describe(desc.trim_end());
    PlanNode {
        kind,
        target,
        startup_cost: 0.0,
        total_cost: cost,
        rows,
        actual_ms,
        actual_rows,
        loops,
        conditions,
        children: Vec::new(),
    }
}

fn field<'a>(it: &mut impl Iterator<Item = &'a str>, key: &str) -> Option<f64> {
    it.find_map(|w| w.strip_prefix(key))
        .and_then(|v| v.parse().ok())
}

/// 末尾の括弧の組を、(それより前, 中身) に分ける。
fn split_last_group(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_end();
    if !s.ends_with(')') {
        return None;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[..i], &s[i + 1..s.len() - 1]));
                }
            }
            _ => {}
        }
    }
    None
}

/// `Filter: (o.total > 10)`、`Index lookup on o using cid (cid = c.id)`、`Table scan on o` を
/// (種類, 対象, 条件)に分ける。
fn describe(desc: &str) -> (String, Option<String>, Vec<(String, String)>) {
    if let Some((head, cond)) = desc.split_once(": ") {
        return (
            head.to_string(),
            None,
            vec![("条件".into(), cond.to_string())],
        );
    }
    let mut conditions = Vec::new();
    let mut rest = desc.to_string();
    if let Some((head, group)) = split_last_group(&rest) {
        conditions.push(("条件".to_string(), group.to_string()));
        rest = head.trim_end().to_string();
    }
    match rest.split_once(" on ") {
        Some((kind, target)) => (kind.to_string(), Some(target.to_string()), conditions),
        None => (rest, None, conditions),
    }
}

/// MariaDB の `EXPLAIN FORMAT=JSON` / `ANALYZE FORMAT=JSON`(`query_block` から始まる)。
pub fn parse_mariadb_json(json: &str) -> Result<Plan, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("実行計画の JSON を読めません: {e}"))?;
    let block = v
        .get("query_block")
        .ok_or("実行計画に query_block がありません")?;
    let mut root = container("Query block", block, false);
    root.total_cost = block
        .get("cost")
        .and_then(Value::as_f64)
        .unwrap_or(root.total_cost);
    // ブロックの時間は、全体(最適化や結果の送出を含む)。ノードの重さの比べ合いに混ぜないよう、
    // ルートの時間は子の合計にして、全体は `execution_ms` に持たせる
    let execution_ms = root.actual_ms;
    if root.children.iter().all(|c| c.actual_ms.is_some()) {
        root.actual_ms = Some(root.children.iter().filter_map(|c| c.actual_ms).sum());
    }
    let planning_ms = v
        .pointer("/query_optimization/r_total_time_ms")
        .and_then(Value::as_f64);
    Ok(Plan {
        execution_ms,
        planning_ms,
        root,
    })
}

/// 子を持つ入れ物のキーと、画面に出す名前。
const CONTAINERS: [(&str, &str); 9] = [
    ("filesort", "Sort"),
    ("temporary_table", "Temporary table"),
    ("duplicates_removal", "Duplicates removal"),
    ("materialization", "Materialization"),
    ("read_sorted_file", "Read sorted file"),
    ("window_functions_computation", "Window functions"),
    ("block-nl-join", "Block nested loop join"),
    ("union_result", "Union"),
    ("grouping_operation", "Group"),
];

/// 入れ物のノード。子は、値の中の `table`、入れ物、`nested_loop`、サブクエリから集める。
/// `exclusive` は、`r_total_time_ms` がそのノード自身の分だけ(子を含まない)であること
/// (`filesort` など)。含むなら、そのまま全体の時間になる。
fn container(kind: &str, v: &Value, exclusive: bool) -> PlanNode {
    let mut node = PlanNode {
        kind: kind.to_string(),
        target: None,
        startup_cost: 0.0,
        total_cost: 0.0,
        rows: v
            .get("r_output_rows")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        actual_ms: v.get("r_total_time_ms").and_then(Value::as_f64),
        actual_rows: v.get("r_output_rows").and_then(Value::as_f64),
        loops: v.get("r_loops").and_then(Value::as_f64).unwrap_or(1.0),
        conditions: Vec::new(),
        children: Vec::new(),
    };
    for (key, label) in [
        ("sort_key", "Sort Key"),
        ("join_condition", "Join Condition"),
    ] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            node.conditions.push((label.to_string(), s.to_string()));
        }
    }
    let Some(obj) = v.as_object() else {
        return node;
    };
    for (key, value) in obj {
        match key.as_str() {
            "table" => node.children.push(table(value)),
            "nested_loop" => node.children.push(nested_loop(value)),
            "subqueries" | "query_specifications" => {
                for item in value.as_array().into_iter().flatten() {
                    if let Some(b) = item.get("query_block") {
                        node.children.push(container("Subquery", b, false));
                    }
                }
            }
            k => {
                if let Some((_, label)) = CONTAINERS.iter().find(|(c, _)| *c == k) {
                    node.children.push(container(label, value, true));
                }
            }
        }
    }
    // 自分の分の見積もりは持たない(子の合計)。時間が無い入れ物は、子の合計にして、自分の分を 0 にする
    node.total_cost = node.children.iter().map(|c| c.total_cost).sum();
    if node.children.iter().all(|c| c.actual_ms.is_some()) {
        let kids: f64 = node.children.iter().filter_map(|c| c.actual_ms).sum();
        node.actual_ms = match node.actual_ms {
            Some(own) if exclusive => Some(own + kids),
            Some(total) => Some(total),
            None => Some(kids),
        };
    }
    node
}

fn nested_loop(v: &Value) -> PlanNode {
    let mut node = container("Nested loop", &Value::Null, true);
    for item in v.as_array().into_iter().flatten() {
        if let Some(t) = item.get("table") {
            node.children.push(table(t));
        } else if let Some(j) = item.get("block-nl-join") {
            node.children
                .push(container("Block nested loop join", j, true));
        }
    }
    node.total_cost = node.children.iter().map(|c| c.total_cost).sum();
    if node.children.iter().all(|c| c.actual_ms.is_some()) {
        node.actual_ms = Some(node.children.iter().filter_map(|c| c.actual_ms).sum());
    }
    node
}

fn table(v: &Value) -> PlanNode {
    let num = |k: &str| v.get(k).and_then(Value::as_f64);
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let access = text("access_type").unwrap_or_default();
    let kind = match access.as_str() {
        "ALL" => "Full table scan".to_string(),
        "index" => "Full index scan".to_string(),
        "range" => "Index range scan".to_string(),
        "ref" | "ref_or_null" => "Index lookup".to_string(),
        "eq_ref" | "const" | "system" => "Single-row index lookup".to_string(),
        "" => "Table access".to_string(),
        other => format!("Table access ({other})"),
    };
    let name = text("table_name").unwrap_or_default();
    let target = match text("key") {
        Some(k) => Some(format!("{name} using {k}")),
        None => Some(name).filter(|n| !n.is_empty()),
    };
    let mut conditions = Vec::new();
    if let Some(c) = text("attached_condition") {
        conditions.push(("条件".to_string(), c));
    }
    let loops = num("r_loops").or_else(|| num("loops")).unwrap_or(1.0);
    // 実測の時間は、表の読み取りと、それ以外(条件の評価など)の合計
    let actual_ms = match (num("r_table_time_ms"), num("r_other_time_ms")) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    PlanNode {
        kind,
        target,
        startup_cost: 0.0,
        total_cost: num("cost").unwrap_or(0.0),
        rows: num("rows").unwrap_or(0.0),
        actual_ms,
        actual_rows: num("r_rows").map(|r| r * loops),
        loops,
        conditions,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "-> Sort: `SUM(o.total)`  (actual time=0.225..0.225 rows=2 loops=1)
    -> Table scan on <temporary>  (actual time=0.218..0.219 rows=2 loops=1)
        -> Aggregate using temporary table  (actual time=0.218..0.218 rows=2 loops=1)
            -> Nested loop inner join  (cost=0.9 rows=1) (actual time=0.0151..0.0186 rows=2 loops=1)
                -> Filter: ((o.total > 10.00) and (o.cid is not null))  (cost=0.55 rows=1) (actual time=0.0101..0.0115 rows=2 loops=1)
                    -> Table scan on o  (cost=0.55 rows=3) (actual time=0.00662..0.00817 rows=3 loops=1)
                -> Single-row index lookup on c using PRIMARY (id = o.cid)  (cost=0.35 rows=1) (actual time=0.0029..0.00296 rows=1 loops=2)
";

    #[test]
    fn the_indented_tree_becomes_a_tree_of_nodes() {
        let p = parse_mysql_tree(TREE).unwrap();
        assert_eq!(p.root.kind, "Sort");
        let join = &p.root.children[0].children[0].children[0];
        assert_eq!(join.kind, "Nested loop inner join");
        assert_eq!((join.total_cost, join.rows), (0.9, 1.0));
        assert_eq!(
            join.children.len(),
            2,
            "the filter and the lookup are siblings"
        );
        let filter = &join.children[0];
        assert_eq!(filter.kind, "Filter");
        assert_eq!(
            filter.conditions[0].1,
            "((o.total > 10.00) and (o.cid is not null))"
        );
        assert_eq!(filter.children[0].kind, "Table scan");
        assert_eq!(filter.children[0].target.as_deref(), Some("o"));
        let lookup = &join.children[1];
        assert_eq!(lookup.kind, "Single-row index lookup");
        assert_eq!(lookup.target.as_deref(), Some("c using PRIMARY"));
        assert_eq!(lookup.conditions[0].1, "id = o.cid");
    }

    #[test]
    fn analyze_times_are_multiplied_by_loops() {
        let p = parse_mysql_tree(TREE).unwrap();
        let lookup = &p.root.children[0].children[0].children[0].children[1];
        assert_eq!(lookup.loops, 2.0);
        assert_eq!(lookup.actual_rows, Some(2.0));
        assert!((lookup.actual_ms.unwrap() - 0.00592).abs() < 1e-9);
        assert_eq!(p.execution_ms, Some(0.225));
    }

    #[test]
    fn a_plan_without_analyze_has_estimates_only() {
        let p = parse_mysql_tree("-> Table scan on t  (cost=0.35 rows=3)\n").unwrap();
        assert_eq!((p.root.total_cost, p.root.rows), (0.35, 3.0));
        assert_eq!(p.root.actual_ms, None);
        assert_eq!(p.execution_ms, None);
    }

    #[test]
    fn never_executed_nodes_and_cost_ranges() {
        let p = parse_mysql_tree("-> Limit: 1 row(s)  (cost=1.5..2.5 rows=1) (never executed)\n")
            .unwrap();
        assert_eq!(p.root.total_cost, 2.5);
        assert_eq!((p.root.loops, p.root.actual_ms), (0.0, Some(0.0)));
    }

    #[test]
    fn malformed_trees_are_reported() {
        assert!(parse_mysql_tree("").is_err());
        assert!(parse_mysql_tree("not a plan").is_err());
    }

    const MARIA: &str = r#"{
      "query_optimization": {"r_total_time_ms": 0.2},
      "query_block": {"select_id": 1, "cost": 0.0148, "r_loops": 1, "r_total_time_ms": 0.37,
        "filesort": {"sort_key": "sum(o.total)", "r_loops": 1, "r_total_time_ms": 0.04, "r_output_rows": 2,
          "temporary_table": {"nested_loop": [
            {"table": {"table_name": "o", "access_type": "ALL", "loops": 1, "r_loops": 1, "rows": 3, "r_rows": 3,
                       "cost": 0.0113, "r_table_time_ms": 0.07, "r_other_time_ms": 0.03,
                       "attached_condition": "o.total > 10"}},
            {"table": {"table_name": "c", "access_type": "eq_ref", "key": "PRIMARY", "loops": 3, "r_loops": 2, "rows": 1,
                       "r_rows": 1, "cost": 0.0035, "r_table_time_ms": 0.01, "r_other_time_ms": 0.0}}]}}}}"#;

    #[test]
    fn mariadb_json_becomes_a_tree() {
        let p = parse_mariadb_json(MARIA).unwrap();
        assert_eq!(p.root.kind, "Query block");
        assert_eq!(p.root.total_cost, 0.0148);
        assert_eq!((p.planning_ms, p.execution_ms), (Some(0.2), Some(0.37)));
        assert_eq!(
            p.root.self_ms(),
            Some(0.0),
            "the block total is not a node's own time"
        );
        let sort = &p.root.children[0];
        assert_eq!(sort.kind, "Sort");
        assert_eq!(
            sort.conditions[0],
            ("Sort Key".into(), "sum(o.total)".into())
        );
        let tmp = &sort.children[0];
        assert_eq!(tmp.kind, "Temporary table");
        let nl = &tmp.children[0];
        assert_eq!(nl.kind, "Nested loop");
        let (o, c) = (&nl.children[0], &nl.children[1]);
        assert_eq!(
            (o.kind.as_str(), o.target.as_deref()),
            ("Full table scan", Some("o"))
        );
        assert_eq!(o.conditions[0].1, "o.total > 10");
        assert!(
            (o.actual_ms.unwrap() - 0.10).abs() < 1e-9,
            "table + other time"
        );
        assert_eq!(c.kind, "Single-row index lookup");
        assert_eq!(c.target.as_deref(), Some("c using PRIMARY"));
        assert_eq!(c.actual_rows, Some(2.0), "rows per loop times loops");
        // 並べ替えの時間は、そのノード自身の分(子の時間は含まない)
        assert!((sort.actual_ms.unwrap() - (0.04 + 0.11)).abs() < 1e-9);
        assert!((sort.self_ms().unwrap() - 0.04).abs() < 1e-9);
        assert_eq!(p.heaviest().kind, "Full table scan");
    }

    #[test]
    fn malformed_mariadb_json_is_reported() {
        assert!(parse_mariadb_json("nope").is_err());
        assert!(
            parse_mariadb_json("{}")
                .unwrap_err()
                .contains("query_block")
        );
    }
}
