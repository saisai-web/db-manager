//! ER 図のためのデータ。表と外部キーの関係を取り出し、表の配置を計算する。
//! DB に依存しない純粋なロジック(関係の向きは、参照する表 → 参照される表)。

use std::collections::{HashMap, HashSet};

use driver_api::KeyKind;

use crate::types::TableDetails;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErColumn {
    pub name: String,
    pub type_name: String,
    pub primary: bool,
    pub foreign: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ErColumn>,
}

/// 外部キー 1 本。`from` が `to` を参照する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErRelation {
    pub name: String,
    /// `tables` の添字
    pub from: usize,
    pub to: usize,
    pub from_columns: Vec<String>,
    pub to_columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErGraph {
    pub tables: Vec<ErTable>,
    pub relations: Vec<ErRelation>,
}

/// `FOREIGN KEY (a) REFERENCES public.t(b, c)` や `REFERENCES t(id)` から、参照先を取り出す。
/// (スキーマ名、表名、列)。引用符は外す。
pub fn parse_reference(definition: &str) -> Option<(Option<String>, String, Vec<String>)> {
    let upper = definition.to_ascii_uppercase();
    let at = upper.find("REFERENCES")? + "REFERENCES".len();
    let rest = definition[at..].trim_start();
    let open = rest.find('(')?;
    let name_part = rest[..open].trim();
    let close = rest[open..].find(')')? + open;
    let cols = rest[open + 1..close]
        .split(',')
        .map(|c| unquote(c.trim()))
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>();
    if name_part.is_empty() || cols.is_empty() {
        return None;
    }
    // schema.table(引用符つきの名前に、ドットが含まれる場合は、考えない)
    let (schema, table) = match name_part.rsplit_once('.') {
        Some((s, t)) => (Some(unquote(s)), unquote(t)),
        None => (None, unquote(name_part)),
    };
    Some((schema, table, cols))
}

fn unquote(s: &str) -> String {
    s.trim()
        .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_string()
}

/// 表の詳細から、ER 図のデータを作る。スキーマの外の表への参照は、関係に含めない。
pub fn build<'a>(details: impl IntoIterator<Item = &'a TableDetails>) -> ErGraph {
    let details: Vec<&TableDetails> = details.into_iter().collect();
    let mut tables: Vec<ErTable> = details
        .iter()
        .map(|d| {
            let pk: HashSet<&str> = d
                .keys
                .iter()
                .filter(|k| k.kind == KeyKind::Primary)
                .flat_map(|k| k.columns.iter().map(String::as_str))
                .collect();
            let fk: HashSet<&str> = d
                .keys
                .iter()
                .filter(|k| k.kind == KeyKind::Foreign)
                .flat_map(|k| k.columns.iter().map(String::as_str))
                .collect();
            ErTable {
                schema: d.table.schema.clone(),
                name: d.table.name.clone(),
                columns: d
                    .columns
                    .iter()
                    .map(|c| ErColumn {
                        name: c.name.clone(),
                        type_name: c.type_name.clone(),
                        primary: pk.contains(c.name.as_str()),
                        foreign: fk.contains(c.name.as_str()),
                    })
                    .collect(),
            }
        })
        .collect();
    tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    let index: HashMap<(&str, &str), usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| ((t.schema.as_str(), t.name.as_str()), i))
        .collect();

    let mut relations = Vec::new();
    for d in &details {
        let from = index[&(d.table.schema.as_str(), d.table.name.as_str())];
        for k in d.keys.iter().filter(|k| k.kind == KeyKind::Foreign) {
            let Some((schema, table, to_columns)) =
                k.definition.as_deref().and_then(parse_reference)
            else {
                continue;
            };
            let schema = schema.unwrap_or_else(|| d.table.schema.clone());
            if let Some(&to) = index.get(&(schema.as_str(), table.as_str())) {
                relations.push(ErRelation {
                    name: k.name.clone(),
                    from,
                    to,
                    from_columns: k.columns.clone(),
                    to_columns,
                });
            }
        }
    }
    relations.sort_by(|a, b| (a.from, a.to, &a.name).cmp(&(b.from, b.to, &b.name)));
    ErGraph { tables, relations }
}

/// 表の位置(左上の座標)と、全体の大きさ。
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub positions: Vec<(f32, f32)>,
    pub size: (f32, f32),
}

pub const BOX_WIDTH: f32 = 220.0;
pub const ROW_HEIGHT: f32 = 18.0;
pub const HEADER_HEIGHT: f32 = 26.0;
pub const GAP_X: f32 = 90.0;
const GAP_Y: f32 = 30.0;
/// 表 1 つの箱の高さに含める列の最大数(多い表は、省略する)
pub const MAX_COLUMNS_SHOWN: usize = 12;

pub fn box_height(table: &ErTable) -> f32 {
    HEADER_HEIGHT + ROW_HEIGHT * table.columns.len().min(MAX_COLUMNS_SHOWN) as f32 + 6.0
}

/// 箱の上端から、列の行の中央までの距離。省略されて見えない列は、箱の下端。
pub fn column_anchor(table: &ErTable, column: &str) -> f32 {
    match table.columns.iter().position(|c| c.name == column) {
        Some(i) if i < MAX_COLUMNS_SHOWN => HEADER_HEIGHT + ROW_HEIGHT * (i as f32 + 0.5),
        _ => box_height(table),
    }
}

/// 参照される表を左に、参照する表を右に置く(層に分け、層ごとに縦に並べる)。
/// 循環があっても、必ず終わる。
pub fn layout(graph: &ErGraph) -> Layout {
    let n = graph.tables.len();
    // 各表の層 = 自分が参照する表の層の最大値 + 1(参照がなければ 0)
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for r in graph.relations.iter().filter(|r| r.from != r.to) {
            if layer[r.from] <= layer[r.to] && layer[r.to] + 1 < n {
                layer[r.from] = layer[r.to] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let layers = layer.iter().copied().max().map_or(0, |m| m + 1);
    let mut y_in_layer = vec![0.0f32; layers];
    let mut positions = vec![(0.0, 0.0); n];
    let mut height = 0.0f32;
    for (i, t) in graph.tables.iter().enumerate() {
        let l = layer[i];
        positions[i] = (l as f32 * (BOX_WIDTH + GAP_X), y_in_layer[l]);
        y_in_layer[l] += box_height(t) + GAP_Y;
        height = height.max(y_in_layer[l]);
    }
    Layout {
        positions,
        size: (
            layers as f32 * (BOX_WIDTH + GAP_X) - GAP_X.min(layers as f32 * GAP_X),
            height,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::{ColumnInfo, KeyInfo, TableRef};

    fn details(schema: &str, name: &str, cols: &[&str], keys: Vec<KeyInfo>) -> TableDetails {
        TableDetails {
            table: TableRef::new(schema, name),
            columns: cols
                .iter()
                .enumerate()
                .map(|(i, c)| ColumnInfo {
                    name: c.to_string(),
                    type_name: "int".into(),
                    nullable: true,
                    default: None,
                    ordinal: i as u32 + 1,
                })
                .collect(),
            keys,
            indexes: vec![],
        }
    }

    fn pk(cols: &[&str]) -> KeyInfo {
        KeyInfo {
            name: "pk".into(),
            kind: KeyKind::Primary,
            columns: cols.iter().map(|s| s.to_string()).collect(),
            definition: None,
        }
    }

    fn fk(name: &str, cols: &[&str], def: &str) -> KeyInfo {
        KeyInfo {
            name: name.into(),
            kind: KeyKind::Foreign,
            columns: cols.iter().map(|s| s.to_string()).collect(),
            definition: Some(def.into()),
        }
    }

    #[test]
    fn parses_references_in_postgres_and_sqlite_forms() {
        assert_eq!(
            parse_reference(
                "FOREIGN KEY (customer_id) REFERENCES app.customers(id) ON DELETE CASCADE"
            ),
            Some((Some("app".into()), "customers".into(), vec!["id".into()]))
        );
        assert_eq!(
            parse_reference("REFERENCES parent(id)"),
            Some((None, "parent".into(), vec!["id".into()]))
        );
        assert_eq!(
            parse_reference("FOREIGN KEY (a, b) REFERENCES \"My Schema\".\"T\"(x, y)"),
            Some((
                Some("My Schema".into()),
                "T".into(),
                vec!["x".into(), "y".into()]
            ))
        );
        assert_eq!(parse_reference("CHECK (a > 0)"), None);
        assert_eq!(parse_reference("REFERENCES ()"), None);
    }

    fn sample() -> Vec<TableDetails> {
        vec![
            details(
                "public",
                "orders",
                &["id", "customer_id"],
                vec![
                    pk(&["id"]),
                    fk(
                        "orders_customer_fk",
                        &["customer_id"],
                        "FOREIGN KEY (customer_id) REFERENCES customers(id)",
                    ),
                ],
            ),
            details("public", "customers", &["id", "name"], vec![pk(&["id"])]),
            details(
                "public",
                "items",
                &["order_id", "sku"],
                vec![
                    fk(
                        "items_order_fk",
                        &["order_id"],
                        "FOREIGN KEY (order_id) REFERENCES public.orders(id)",
                    ),
                    fk(
                        "external",
                        &["sku"],
                        "FOREIGN KEY (sku) REFERENCES other.skus(id)",
                    ),
                ],
            ),
        ]
    }

    #[test]
    fn builds_tables_sorted_with_key_marks_and_in_schema_relations_only() {
        let g = build(&sample());
        assert_eq!(
            g.tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["customers", "items", "orders"]
        );
        let orders = g.tables.iter().find(|t| t.name == "orders").unwrap();
        assert!(orders.columns[0].primary && !orders.columns[0].foreign);
        assert!(orders.columns[1].foreign && !orders.columns[1].primary);
        // 別のスキーマ(other)への参照は、含まれない
        assert_eq!(g.relations.len(), 2);
        let name = |i: usize| g.tables[i].name.as_str();
        let rel: Vec<_> = g
            .relations
            .iter()
            .map(|r| (name(r.from), name(r.to)))
            .collect();
        assert_eq!(rel, [("items", "orders"), ("orders", "customers")]);
    }

    #[test]
    fn referenced_tables_go_left_and_referencing_tables_go_right() {
        let g = build(&sample());
        let l = layout(&g);
        let x = |name: &str| l.positions[g.tables.iter().position(|t| t.name == name).unwrap()].0;
        assert!(
            x("customers") < x("orders") && x("orders") < x("items"),
            "{:?}",
            l.positions
        );
        assert!(l.size.0 > 0.0 && l.size.1 > 0.0);
    }

    #[test]
    fn tables_in_one_layer_do_not_overlap_vertically() {
        let d: Vec<_> = (0..5)
            .map(|i| details("public", &format!("t{i}"), &["a", "b"], vec![]))
            .collect();
        let g = build(&d);
        let l = layout(&g);
        let mut ys: Vec<f32> = l.positions.iter().map(|p| p.1).collect();
        ys.sort_by(f32::total_cmp);
        assert!(
            ys.windows(2)
                .all(|w| w[1] - w[0] >= box_height(&g.tables[0]))
        );
        assert!(
            l.positions.iter().all(|p| p.0 == 0.0),
            "no relations: a single layer"
        );
    }

    #[test]
    fn cycles_and_self_references_terminate() {
        let d = vec![
            details(
                "public",
                "a",
                &["id", "b_id"],
                vec![fk("a_b", &["b_id"], "REFERENCES b(id)")],
            ),
            details(
                "public",
                "b",
                &["id", "a_id", "parent"],
                vec![
                    fk("b_a", &["a_id"], "REFERENCES a(id)"),
                    fk("b_self", &["parent"], "REFERENCES b(id)"),
                ],
            ),
        ];
        let g = build(&d);
        assert_eq!(g.relations.len(), 3);
        let l = layout(&g);
        assert_eq!(l.positions.len(), 2);
        assert!(
            l.positions
                .iter()
                .all(|p| p.0.is_finite() && p.1.is_finite())
        );
    }

    #[test]
    fn anchors_point_at_the_column_row_or_the_bottom_when_hidden() {
        let cols: Vec<String> = (0..15).map(|i| format!("c{i}")).collect();
        let refs: Vec<&str> = cols.iter().map(String::as_str).collect();
        let g = build(&[details("public", "wide", &refs, vec![])]);
        let t = &g.tables[0];
        assert_eq!(column_anchor(t, "c0"), HEADER_HEIGHT + ROW_HEIGHT / 2.0);
        assert_eq!(column_anchor(t, "c14"), box_height(t), "hidden column");
        assert_eq!(column_anchor(t, "missing"), box_height(t));
    }

    #[test]
    fn an_empty_schema_gives_an_empty_diagram() {
        let g = build(&Vec::<TableDetails>::new());
        assert!(g.tables.is_empty());
        assert_eq!(layout(&g).positions.len(), 0);
    }
}
