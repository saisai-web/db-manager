use super::*;
use crate::dsl::parse;

const SHOP: &str = "\
customers \"顧客\"
  id int pk
  name varchar(100) not null \"氏名\"
orders
  id int pk
  customer_id int not null -> customers.id
  coupon_id int -> coupons.id
coupons
  id int pk
items
  order_id int pk -> orders.id
  line int pk
profiles
  customer_id int pk -> customers.id
lonely_a
  id int pk
lonely_b
  id int pk
";

fn design() -> crate::Design {
    parse(SHOP).unwrap()
}

fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

#[test]
fn parents_are_laid_out_left_of_their_children() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    let x = |n: &str| g.rects[d.table_index(n).unwrap()].x;
    assert!(x("customers") < x("orders") && x("orders") < x("items"));
    assert!(x("coupons") < x("orders"));
}

#[test]
fn no_two_boxes_overlap_and_everything_fits_in_the_size() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    for (i, a) in g.rects.iter().enumerate() {
        for b in &g.rects[i + 1..] {
            assert!(!overlap(a, b), "{a:?} {b:?}");
        }
        assert!(a.x >= 0.0 && a.y >= 0.0);
        assert!(a.right() <= g.size.0 && a.bottom() <= g.size.1);
    }
}

#[test]
fn unrelated_tables_go_below_the_connected_ones() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    let r = |n: &str| g.rects[d.table_index(n).unwrap()];
    let connected_bottom = ["customers", "orders", "coupons", "items"]
        .iter()
        .map(|n| r(n).bottom())
        .fold(0.0, f32::max);
    assert!(r("lonely_a").y > connected_bottom);
}

#[test]
fn manual_positions_win_and_new_tables_do_not_land_on_them() {
    let mut d = design();
    d.tables[0].pos = Some((500.0, 500.0));
    let g = geometry(&d, Labels::Physical);
    assert_eq!((g.rects[0].x, g.rects[0].y), (500.0, 500.0));
    for (i, a) in g.rects.iter().enumerate() {
        for b in &g.rects[i + 1..] {
            assert!(!overlap(a, b), "{a:?} {b:?}");
        }
    }
    assert!(
        g.rects[1..].iter().all(|r| r.y >= g.rects[0].bottom()),
        "free tables start below fixed ones"
    );
}

#[test]
fn edges_run_from_the_child_column_row_to_the_parent_column_row() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    let ci = d.table_index("orders").unwrap();
    let pi = d.table_index("customers").unwrap();
    let e = g
        .edges
        .iter()
        .find(|e| e.child == ci && e.parent == pi)
        .unwrap();
    let first = e.points[0];
    let last = *e.points.last().unwrap();
    // customer_id は 2 行目、id は 1 行目
    assert_eq!(first.1, g.rects[ci].y + row_center(1, 1.0));
    assert_eq!(last.1, g.rects[pi].y + row_center(0, 1.0));
    assert_eq!(
        first.0, g.rects[ci].x,
        "leaves from the left edge toward the parent"
    );
    assert_eq!(last.0, g.rects[pi].right());
    assert_eq!((e.child_dir, e.parent_dir), (-1.0, 1.0));
    // 折れ線は縦横だけ
    for w in e.points.windows(2) {
        assert!(w[0].0 == w[1].0 || w[0].1 == w[1].1, "{:?}", e.points);
    }
}

#[test]
fn markers_follow_nullability_and_uniqueness() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    let find = |c: &str, p: &str| {
        let (ci, pi) = (d.table_index(c).unwrap(), d.table_index(p).unwrap());
        g.edges
            .iter()
            .find(|e| e.child == ci && e.parent == pi)
            .unwrap()
            .clone()
    };
    let required = find("orders", "customers");
    assert_eq!(
        (required.parent_marker, required.child_marker),
        (Marker::One, Marker::Many)
    );
    assert_eq!(
        find("orders", "coupons").parent_marker,
        Marker::ZeroOrOne,
        "nullable fk"
    );
    assert_eq!(
        find("profiles", "customers").child_marker,
        Marker::ZeroOrOne,
        "fk is the whole key: one to one"
    );
    assert_eq!(
        find("items", "orders").child_marker,
        Marker::Many,
        "fk is part of a composite key"
    );
}

#[test]
fn self_references_and_overlapping_boxes_loop_around_the_right_side() {
    let d = parse("emp\n  id int pk\n  boss int -> emp.id\n").unwrap();
    let g = geometry(&d, Labels::Physical);
    let e = &g.edges[0];
    assert_eq!(e.points.len(), 4);
    assert!(e.points[1].0 > g.rects[0].right());
    assert_ne!(
        e.points[0].1, e.points[3].1,
        "the two ends are on different rows"
    );
    assert!(g.size.0 >= e.points[1].0);

    let d2 = parse("p @0,0\n  id int pk\nc @50,0\n  p int -> p.id\n").unwrap();
    let g2 = geometry(&d2, Labels::Physical);
    assert_eq!((g2.edges[0].child_dir, g2.edges[0].parent_dir), (1.0, 1.0));
}

#[test]
fn logical_labels_change_box_widths_and_fall_back_to_physical_names() {
    let d = design();
    let phys = box_size(&d.tables[0], Labels::Physical).0;
    let logical = box_size(&d.tables[0], Labels::Logical).0;
    assert!(phys >= MIN_WIDTH && logical >= MIN_WIDTH);
    assert_eq!(header_label(&d.tables[0], Labels::Logical).0, "顧客");
    assert_eq!(header_label(&d.tables[1], Labels::Logical).0, "orders");
    assert_eq!(
        column_label(&d.tables[0].columns[1], Labels::Logical),
        ("氏名".into(), String::new())
    );
    let (n, ty) = column_label(&d.tables[0].columns[1], Labels::Physical);
    assert_eq!((n.as_str(), ty.as_str()), ("name", "varchar(100)"));
}

#[test]
fn wide_text_widens_the_box() {
    let narrow = parse("t\n  a int\n").unwrap();
    let wide = parse("t\n  a_very_long_column_name_that_goes_on varchar(255)\n").unwrap();
    assert!(
        box_size(&wide.tables[0], Labels::Physical).0
            > box_size(&narrow.tables[0], Labels::Physical).0
    );
}

#[test]
fn layout_terminates_with_cycles_and_handles_empty_designs() {
    let d = parse("a\n  b int -> b\nb\n  a int -> a\n").unwrap();
    let g = geometry(&d, Labels::Physical);
    assert_eq!(g.rects.len(), 2);
    assert!(g.size.0.is_finite() && g.size.1.is_finite());
    let empty = geometry(&crate::Design::default(), Labels::Physical);
    assert!(empty.rects.is_empty() && empty.size == (0.0, 0.0));
}

#[test]
fn crossing_reduction_keeps_every_table_exactly_once() {
    let mut src = String::new();
    for i in 0..12 {
        src.push_str(&format!("p{i}\n  id int pk\n"));
    }
    for i in 0..12 {
        src.push_str(&format!(
            "c{i}\n  id int pk\n  a int -> p{}.id\n  b int -> p{}.id\n",
            (i * 5) % 12,
            (i * 7 + 3) % 12
        ));
    }
    let d = parse(&src).unwrap();
    let pos = auto_positions(&d, Labels::Physical);
    assert_eq!(pos.len(), 24);
    let g = geometry(&d, Labels::Physical);
    for (i, a) in g.rects.iter().enumerate() {
        for b in &g.rects[i + 1..] {
            assert!(!overlap(a, b));
        }
    }
}

#[test]
fn marker_shapes_point_away_from_the_box() {
    for dir in [1.0f32, -1.0] {
        for m in [Marker::One, Marker::ZeroOrOne, Marker::Many] {
            for s in marker_shapes(m, 100.0, 50.0, dir) {
                let xs = match s {
                    Shape::Line(a, _, b, _) => vec![a, b],
                    Shape::Circle(cx, ..) => vec![cx],
                };
                assert!(xs.iter().all(|x| (x - 100.0) * dir >= 0.0), "{m:?} {s:?}");
            }
        }
    }
    assert_eq!(marker_shapes(Marker::Many, 0.0, 0.0, 1.0).len(), 3);
    assert_eq!(marker_shapes(Marker::One, 0.0, 0.0, 1.0).len(), 2);
}

#[test]
fn vertical_runs_of_different_edges_do_not_share_a_line() {
    let d = design();
    let g = geometry(&d, Labels::Physical);
    let runs: Vec<(f32, f32, f32)> = g
        .edges
        .iter()
        .filter(|e| e.points.len() == 4)
        .map(|e| {
            let (a, b) = (e.points[1].1, e.points[2].1);
            (e.points[1].0, a.min(b), a.max(b))
        })
        .collect();
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
            let overlap = a.1 < b.2 && b.1 < a.2;
            assert!(!(overlap && (a.0 - b.0).abs() < 7.0), "{a:?} {b:?}");
        }
    }
}

#[test]
fn a_table_scale_resizes_the_box_and_moves_its_relation_anchors() {
    let mut d = design();
    let ci = d.table_index("orders").unwrap();
    let pi = d.table_index("customers").unwrap();
    let plain = geometry(&d, Labels::Physical);
    d.tables[ci].scale = Some(2.0);
    let big = geometry(&d, Labels::Physical);
    let (a, b) = (plain.rects[ci], big.rects[ci]);
    // 幅と、見出しの下の高さは 2 倍になる(見出しの高さは変わらない)
    let rows = a.h - HEADER_H - FOOT_H;
    assert_eq!((b.w, b.h), (a.w * 2.0, HEADER_H + rows * 2.0 + FOOT_H));
    // 線は、拡大した箱の同じ列の行(の中央)から出る
    let e = |g: &Geometry| {
        g.edges
            .iter()
            .find(|e| e.child == ci && e.parent == pi)
            .unwrap()
            .points[0]
            .1
    };
    // 行の高さは、ほんの少ししか増えない(増えた分の大半は、行の下の余白)
    let grow = row_height(2.0) / row_height(1.0);
    assert!(grow > 1.0 && grow < 1.3, "{grow}");
    let (a, b) = (
        e(&plain) - plain.rects[ci].y - HEADER_H,
        e(&big) - big.rects[ci].y - HEADER_H,
    );
    assert!((b - a * grow).abs() < 1e-3, "{a} {b}");
    // 範囲外の値は、範囲に収める
    d.tables[ci].scale = Some(99.0);
    assert_eq!(d.tables[ci].factor(), crate::model::MAX_SCALE);
}

#[test]
fn a_loose_table_listed_before_connected_ones_does_not_break_the_layout() {
    // つながりのない表(2 番目)が、つながった表(3・4 番目)より前にあっても、配置できる
    let d =
        parse("a @0,0\n  id int pk\nloose\n  id int pk\nb\n  id int pk -> c.id\nc\n  id int pk\n")
            .unwrap();
    let g = geometry(&d, Labels::Physical);
    assert_eq!(g.rects.len(), 4);
    assert_eq!(g.edges.len(), 1);
    // 先頭の表だけ位置を決めて、残りは自動に任せる(表を足した直後の状態)
    let positions = resolve_positions(&d, Labels::Physical);
    assert_eq!(positions.len(), 4);
    assert_eq!(positions[0], (0.0, 0.0));
}

#[test]
fn every_order_of_connected_and_loose_tables_can_be_laid_out() {
    let lines = [
        "p\n  id int pk\n",
        "c\n  id int pk\n  p_id int -> p.id\n",
        "x\n  id int pk\n",
        "y\n  id int pk\n",
    ];
    for mask in 0..24usize {
        // 4 つの表の並べ替え(mask 番目の順列)
        let mut idx: Vec<usize> = (0..4).collect();
        let mut m = mask;
        for i in (1..4).rev() {
            idx.swap(i, m % (i + 1));
            m /= i + 1;
        }
        let text: String = idx.iter().map(|&i| lines[i]).collect();
        let d = parse(&text).unwrap();
        assert_eq!(geometry(&d, Labels::Physical).rects.len(), 4, "{text}");
    }
}
