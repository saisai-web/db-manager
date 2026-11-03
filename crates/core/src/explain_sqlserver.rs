//! SQL Server の実行計画(`SET SHOWPLAN_XML` / `SET STATISTICS XML` の XML)の、木構造への変換。
//! DB に依存しない純粋なロジック。
//!
//! 計画は `RelOp` の入れ子で、子の `RelOp` は、演算子ごとの要素(`NestedLoops`、`IndexScan` など)の
//! 中にある。要素の名前は演算子で変わるので、`RelOp` に当たるまで要素を辿って、子を集める。

use roxmltree::{Document, Node};

use crate::explain::{Plan, PlanNode};

pub fn parse_sqlserver_xml(xml: &str) -> Result<Plan, String> {
    let doc = Document::parse(xml).map_err(|e| format!("実行計画の XML を読めません: {e}"))?;
    let query_plan = doc
        .descendants()
        .find(|n| n.has_tag_name("QueryPlan"))
        .ok_or("実行計画に QueryPlan がありません")?;
    let root = child_relops(query_plan)
        .into_iter()
        .next()
        .ok_or("実行計画に RelOp がありません")?;
    let root = node(root);
    Ok(Plan {
        planning_ms: query_plan
            .attribute("CompileTime")
            .and_then(|t| t.parse().ok()),
        execution_ms: root.actual_ms,
        root,
    })
}

/// `parent` の直下にある演算子(ほかの `RelOp` の中は辿らない)。
fn child_relops<'a, 'i>(parent: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let mut found = Vec::new();
    let mut stack: Vec<Node> = parent.children().filter(Node::is_element).collect();
    stack.reverse();
    while let Some(n) = stack.pop() {
        if n.has_tag_name("RelOp") {
            found.push(n);
        } else {
            stack.extend(n.children().filter(Node::is_element).rev());
        }
    }
    found
}

fn node(op: Node) -> PlanNode {
    let num = |name: &str| op.attribute(name).and_then(|v| v.parse::<f64>().ok());
    let runtime: Vec<Node> = op
        .descendants()
        .filter(|n| n.has_tag_name("RunTimeCountersPerThread") && own(op, *n))
        .collect();
    let sum = |name: &str| {
        runtime
            .iter()
            .filter_map(|n| n.attribute(name)?.parse::<f64>().ok())
            .sum::<f64>()
    };
    let (actual_ms, actual_rows, loops) = if runtime.is_empty() {
        (None, None, 1.0)
    } else {
        let ms = runtime
            .iter()
            .filter_map(|n| n.attribute("ActualElapsedms")?.parse::<f64>().ok())
            .fold(0.0, f64::max);
        (Some(ms), Some(sum("ActualRows")), sum("ActualExecutions"))
    };
    let children: Vec<PlanNode> = child_relops(op).into_iter().map(node).collect();
    // `Compute Scalar` などは、実測の数え上げを持たない。1 つの子の行をそのまま通すので、子の値を引き継ぐ
    // (自分の分の時間は 0 になる)。
    let (actual_ms, actual_rows, loops) = match children.as_slice() {
        [only] if runtime.is_empty() && only.actual_ms.is_some() => {
            (only.actual_ms, only.actual_rows, only.loops)
        }
        _ => (actual_ms, actual_rows, loops),
    };
    PlanNode {
        kind: op.attribute("PhysicalOp").unwrap_or("(不明)").to_string(),
        target: target(op),
        startup_cost: 0.0,
        total_cost: num("EstimatedTotalSubtreeCost").unwrap_or(0.0),
        rows: num("EstimateRows").unwrap_or(0.0),
        actual_ms,
        actual_rows,
        loops,
        conditions: conditions(op),
        children,
    }
}

/// `n` が、`op` 自身のもの(子の `RelOp` の中ではない)。
fn own(op: Node, n: Node) -> bool {
    n.ancestors()
        .skip(1)
        .find(|a| a.has_tag_name("RelOp"))
        .is_some_and(|a| a == op)
}

/// 対象のテーブルと索引(`orders using PK_orders`)。`dbo` 以外のスキーマは付ける。
fn target(op: Node) -> Option<String> {
    let object = op
        .descendants()
        .find(|n| n.has_tag_name("Object") && own(op, *n))?;
    let strip = |v: Option<&str>| v.map(|s| s.trim_matches(|c| c == '[' || c == ']').to_string());
    let table = strip(object.attribute("Table"))?;
    let table = match strip(object.attribute("Schema")).filter(|s| s != "dbo") {
        Some(schema) => format!("{schema}.{table}"),
        None => table,
    };
    Some(match strip(object.attribute("Index")) {
        Some(index) => format!("{table} using {index}"),
        None => table,
    })
}

/// ノードの条件として見せる要素と、その見出し。
const CONDITIONS: [(&str, &str); 4] = [
    ("Predicate", "Predicate"),
    ("SeekPredicates", "Seek Predicate"),
    ("Residual", "Residual"),
    ("ProbeResidual", "Probe Residual"),
];

fn conditions(op: Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (tag, label) in CONDITIONS {
        for holder in op
            .descendants()
            .filter(|n| n.has_tag_name(tag) && own(op, *n))
        {
            // 入れ子の式のうち、いちばん外側の式だけ(中の部分式は、その一部)
            let text: Vec<&str> = holder
                .descendants()
                .filter(|n| {
                    n.has_tag_name("ScalarOperator") && n.attribute("ScalarString").is_some()
                })
                .filter(|n| {
                    !n.ancestors().skip(1).take_while(|a| *a != holder).any(|a| {
                        a.has_tag_name("ScalarOperator") && a.attribute("ScalarString").is_some()
                    })
                })
                .filter_map(|n| n.attribute("ScalarString"))
                .collect();
            if !text.is_empty() {
                out.push((label.to_string(), text.join(", ")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実機(SQL Server 2022)の推定の計画を、名前空間と主要な要素を残して縮めたもの。
    const ESTIMATED: &str = r#"<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.564"><BatchSequence><Batch><Statements><StmtSimple StatementText="select a.v, count(*) from a join b on b.aid=a.id where b.w &gt; 3 group by a.v"><QueryPlan CachedPlanSize="32" CompileTime="18" CompileCPU="18">
<RelOp NodeId="1" PhysicalOp="Stream Aggregate" LogicalOp="Aggregate" EstimateRows="1" EstimatedTotalSubtreeCost="0.0540424"><OutputList><ColumnReference Column="Expr1007"/></OutputList><StreamAggregate>
<RelOp NodeId="3" PhysicalOp="Merge Join" LogicalOp="Inner Join" EstimateRows="497" EstimatedTotalSubtreeCost="0.0354378"><Merge ManyToMany="0">
<Residual><ScalarOperator ScalarString="[xp].[dbo].[a].[id]=[xp].[dbo].[b].[aid]"><Compare CompareOp="EQ"><ScalarOperator><Identifier><ColumnReference Column="id"/></Identifier></ScalarOperator></Compare></ScalarOperator></Residual>
<RelOp NodeId="4" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="500" EstimatedTotalSubtreeCost="0.00457274"><IndexScan Ordered="1">
<Object Database="[xp]" Schema="[dbo]" Table="[a]" Index="[PK__a__3213E83FC1B14141]" IndexKind="Clustered"/></IndexScan></RelOp>
<RelOp NodeId="5" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="497" EstimatedTotalSubtreeCost="0.00457274"><IndexScan Ordered="1">
<Object Database="[xp]" Schema="[sales]" Table="[b]" Index="[PK__b]" IndexKind="Clustered"/>
<Predicate><ScalarOperator ScalarString="[xp].[dbo].[b].[w]&gt;(3)"><Compare CompareOp="GT"><ScalarOperator><Const ConstValue="(3)"/></ScalarOperator></Compare></ScalarOperator></Predicate></IndexScan></RelOp>
</Merge></RelOp></StreamAggregate></RelOp></QueryPlan></StmtSimple></Statements></Batch></BatchSequence></ShowPlanXML>"#;

    #[test]
    fn the_nested_relops_become_a_tree_with_targets_and_conditions() {
        let p = parse_sqlserver_xml(ESTIMATED).unwrap();
        assert_eq!(p.root.kind, "Stream Aggregate");
        assert_eq!(p.planning_ms, Some(18.0));
        assert_eq!(p.execution_ms, None);
        let join = &p.root.children[0];
        assert_eq!(
            (join.kind.as_str(), join.total_cost, join.rows),
            ("Merge Join", 0.0354378, 497.0)
        );
        assert_eq!(
            join.conditions,
            [(
                "Residual".to_string(),
                "[xp].[dbo].[a].[id]=[xp].[dbo].[b].[aid]".to_string()
            )],
            "only the outermost expression"
        );
        assert_eq!(join.children.len(), 2);
        assert_eq!(
            join.children[0].target.as_deref(),
            Some("a using PK__a__3213E83FC1B14141")
        );
        assert_eq!(
            join.children[1].target.as_deref(),
            Some("sales.b using PK__b"),
            "a schema other than dbo is kept"
        );
        assert_eq!(
            join.children[1].conditions[0],
            ("Predicate".into(), "[xp].[dbo].[b].[w]>(3)".into())
        );
        assert_eq!(
            p.heaviest().kind,
            "Merge Join",
            "its own cost is the largest"
        );
    }

    /// 実測つき: 各演算子に `RunTimeInformation` がある。
    const ACTUAL: &str = r#"<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan"><BatchSequence><Batch><Statements><StmtSimple><QueryPlan CompileTime="3">
<RelOp NodeId="0" PhysicalOp="Nested Loops" EstimateRows="1" EstimatedTotalSubtreeCost="10"><RunTimeInformation>
<RunTimeCountersPerThread Thread="1" ActualRows="3" ActualElapsedms="4" ActualExecutions="1"/>
<RunTimeCountersPerThread Thread="2" ActualRows="2" ActualElapsedms="6" ActualExecutions="1"/>
<RunTimeCountersPerThread Thread="0" ActualRows="0" ActualElapsedms="0" ActualExecutions="1"/></RunTimeInformation><NestedLoops>
<RelOp NodeId="1" PhysicalOp="Index Seek" EstimateRows="1" EstimatedTotalSubtreeCost="2"><RunTimeInformation>
<RunTimeCountersPerThread Thread="0" ActualRows="5" ActualElapsedms="1" ActualExecutions="5"/></RunTimeInformation>
<IndexSeek><Object Schema="[dbo]" Table="[c]" Index="[PK_c]"/></IndexSeek></RelOp></NestedLoops></RelOp>
</QueryPlan></StmtSimple></Statements></Batch></BatchSequence></ShowPlanXML>"#;

    #[test]
    fn runtime_counters_are_summed_over_threads_and_the_time_is_the_slowest_thread() {
        let p = parse_sqlserver_xml(ACTUAL).unwrap();
        assert_eq!(
            (p.root.actual_rows, p.root.actual_ms, p.root.loops),
            (Some(5.0), Some(6.0), 3.0)
        );
        assert_eq!(p.execution_ms, Some(6.0));
        let seek = &p.root.children[0];
        assert_eq!(
            (seek.actual_rows, seek.actual_ms, seek.loops),
            (Some(5.0), Some(1.0), 5.0),
            "the counters of the parent are not mixed in"
        );
        assert_eq!(p.root.self_ms(), Some(5.0));
    }

    #[test]
    fn an_operator_without_counters_takes_over_its_only_childs_values() {
        let xml = r#"<ShowPlanXML><QueryPlan>
<RelOp PhysicalOp="Compute Scalar" EstimatedTotalSubtreeCost="1"><ComputeScalar>
<RelOp PhysicalOp="Table Scan" EstimatedTotalSubtreeCost="1"><RunTimeInformation>
<RunTimeCountersPerThread ActualRows="7" ActualElapsedms="2" ActualExecutions="1"/></RunTimeInformation><TableScan/></RelOp>
</ComputeScalar></RelOp></QueryPlan></ShowPlanXML>"#;
        let p = parse_sqlserver_xml(xml).unwrap();
        assert_eq!(
            (p.root.actual_rows, p.root.actual_ms),
            (Some(7.0), Some(2.0))
        );
        assert_eq!(p.execution_ms, Some(2.0));
        assert_eq!(p.root.self_ms(), Some(0.0));
    }

    #[test]
    fn malformed_input_is_reported() {
        assert!(parse_sqlserver_xml("not xml").is_err());
        assert!(parse_sqlserver_xml("<ShowPlanXML/>").is_err());
        assert!(
            parse_sqlserver_xml("<ShowPlanXML><QueryPlan/></ShowPlanXML>")
                .unwrap_err()
                .contains("RelOp")
        );
    }
}
