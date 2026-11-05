//! 表を作る順序。外部キーの参照先(親)を先に作る。

use std::collections::HashSet;

use crate::model::Design;

pub struct Order {
    /// `Design::tables` の添字を、作る順に
    pub tables: Vec<usize>,
    /// 循環する参照があった
    pub has_cycle: bool,
}

/// 表ごとの、参照先の表(自分自身と、設計にない表は除く)の添字。
fn parents(d: &Design) -> Vec<HashSet<usize>> {
    d.tables
        .iter()
        .enumerate()
        .map(|(i, t)| {
            t.foreign_keys
                .iter()
                .filter_map(|f| d.table_index(&f.ref_table))
                .filter(|&p| p != i)
                .collect()
        })
        .collect()
}

/// 親を先に並べる。循環があるときは、未作成の親が最も少ない表から順に進める(結果は決まった順)。
pub fn creation_order(d: &Design) -> Order {
    let parents = parents(d);
    let mut done = vec![false; d.tables.len()];
    let mut tables = Vec::new();
    let mut has_cycle = false;
    while tables.len() < d.tables.len() {
        let waiting = |i: usize| parents[i].iter().filter(|&&p| !done[p]).count();
        let next = (0..d.tables.len())
            .filter(|&i| !done[i])
            .min_by_key(|&i| (waiting(i), i))
            .expect("a table is left");
        has_cycle |= waiting(next) > 0;
        done[next] = true;
        tables.push(next);
    }
    Order { tables, has_cycle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    fn names(src: &str) -> (Vec<String>, bool) {
        let d = parse(src).unwrap();
        let o = creation_order(&d);
        (
            o.tables.iter().map(|&i| d.tables[i].name.clone()).collect(),
            o.has_cycle,
        )
    }

    #[test]
    fn parents_come_first_and_the_rest_keep_their_order() {
        let (order, cycle) = names(
            "items\n  o int -> orders\norders\n  c int -> customers\ncustomers\n  id int pk\nzzz\n  id int pk\n",
        );
        assert_eq!(order, ["customers", "orders", "items", "zzz"]);
        assert!(!cycle);
    }

    #[test]
    fn self_references_and_unknown_tables_do_not_count() {
        let (order, cycle) = names("a\n  p int -> a\n  q int -> nowhere\n");
        assert_eq!((order, cycle), (vec!["a".to_string()], false));
    }

    #[test]
    fn cycles_still_give_every_table_once() {
        let (order, cycle) = names("a\n  b int -> b\nb\n  a int -> a\nc\n  a int -> a\n");
        assert!(cycle);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, ["a", "b", "c"]);
        assert_eq!(order.last().unwrap(), "c", "c depends on the cycle");
    }
}
