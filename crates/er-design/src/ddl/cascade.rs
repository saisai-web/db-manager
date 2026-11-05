//! SQL Server の連鎖動作(ON DELETE / ON UPDATE の CASCADE・SET NULL・SET DEFAULT)の制限。
//! 自己参照・循環・同じ表へ至る複数の経路を作る外部キーは、作成時にエラーになる
//! (「may cause cycles or multiple cascade paths」)。そのような外部キーを見つける。

use std::collections::HashSet;

use super::order::Order;
use crate::model::{Design, ForeignKey, RefAction};

fn cascades(fk: &ForeignKey) -> bool {
    [fk.on_delete, fk.on_update].iter().any(|a| {
        matches!(
            a,
            RefAction::Cascade | RefAction::SetNull | RefAction::SetDefault
        )
    })
}

/// 親 → 子の連鎖の辺。`paths[a][b]` は、a から b へ至る連鎖の経路の数(a == b は 1)。
struct Graph {
    paths: Vec<Vec<u32>>,
}

impl Graph {
    fn new(n: usize) -> Self {
        let mut paths = vec![vec![0; n]; n];
        for (i, row) in paths.iter_mut().enumerate() {
            row[i] = 1;
        }
        Self { paths }
    }

    /// 辺 `parent → child` を足すと、どこかへ至る経路が 2 本以上になる(循環を含む)か。
    fn would_conflict(&self, parent: usize, child: usize) -> bool {
        let n = self.paths.len();
        if parent == child || self.paths[child][parent] > 0 {
            return true;
        }
        (0..n).any(|r| {
            self.paths[r][parent] > 0
                && (0..n).any(|x| {
                    self.paths[child][x] > 0
                        && self.paths[r][x] + self.paths[r][parent] * self.paths[child][x] > 1
                })
        })
    }

    fn add(&mut self, parent: usize, child: usize) {
        let into_parent: Vec<u32> = self.paths.iter().map(|row| row[parent]).collect();
        let from_child = self.paths[child].clone();
        for (row, to_parent) in self.paths.iter_mut().zip(into_parent) {
            for (cell, below) in row.iter_mut().zip(&from_child) {
                *cell += to_parent * below;
            }
        }
    }
}

/// 連鎖動作を `NO ACTION` に落とさなければならない外部キー(表の名前, `foreign_keys` の添字)。
/// 表を作る順に外部キーを調べ、先に通ったものを優先する。
pub fn sqlserver_rejected(d: &Design, order: &Order) -> HashSet<(String, usize)> {
    let mut graph = Graph::new(d.tables.len());
    let mut rejected = HashSet::new();
    for &ti in &order.tables {
        let t = &d.tables[ti];
        for (fi, fk) in t
            .foreign_keys
            .iter()
            .enumerate()
            .filter(|(_, f)| cascades(f))
        {
            let Some(pi) = d.table_index(&fk.ref_table) else {
                continue;
            };
            if graph.would_conflict(pi, ti) {
                rejected.insert((t.name.clone(), fi));
            } else {
                graph.add(pi, ti);
            }
        }
    }
    rejected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddl::order::creation_order;
    use crate::dsl::parse;

    fn rejected(src: &str) -> Vec<(String, usize)> {
        let d = parse(src).unwrap();
        let mut v: Vec<_> = sqlserver_rejected(&d, &creation_order(&d))
            .into_iter()
            .collect();
        v.sort();
        v
    }

    #[test]
    fn plain_chains_are_fine_but_self_references_are_not() {
        assert!(rejected("a\n  id int pk\nb\n  a int -> a.id on delete cascade\n").is_empty());
        assert_eq!(
            rejected("a\n  id int pk\n  p int -> a.id on delete set null\n"),
            [("a".into(), 0)]
        );
        assert!(
            rejected("a\n  id int pk\n  p int -> a.id\n").is_empty(),
            "NO ACTION is always allowed"
        );
    }

    #[test]
    fn two_cascade_paths_to_one_table_are_refused_after_the_first() {
        // a → b → d と a → c → d で、d への経路が 2 本になる
        let src = "a\n  id int pk\nb\n  a int -> a.id on delete cascade\nc\n  a int -> a.id on delete cascade\nd\n  b int -> b.id on delete cascade\n  c int -> c.id on delete cascade\n";
        assert_eq!(rejected(src), [("d".into(), 1)]);
    }

    #[test]
    fn cycles_are_refused() {
        let src = "x\n  id int pk\n  y int -> y.id on delete cascade\ny\n  id int pk\n  x int -> x.id on delete cascade\n";
        assert_eq!(
            rejected(src).len(),
            1,
            "one of the two is enough to break the cycle"
        );
    }

    #[test]
    fn a_different_table_cascading_into_the_same_child_twice_counts_as_two_paths() {
        let src = "p\n  id int pk\nc\n  a int -> p.id on delete cascade\n  b int -> p.id on delete cascade\n";
        assert_eq!(rejected(src), [("c".into(), 1)]);
    }
}
