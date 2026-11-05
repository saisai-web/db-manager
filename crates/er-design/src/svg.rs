//! ER 図を SVG にする(単独のファイルにも、HTML の中にも使える)。

use std::fmt::Write as _;

use crate::diagram::{
    BADGE_W, HEADER_H, HEADER_SCALE, Labels, MIN_WIDTH, PAD, Shape, column_label, geometry,
    header_label, marker_shapes, row_center, row_height, text_width,
};
use crate::model::Design;

const MARGIN: f32 = 24.0;
const FONT: &str = "'Hiragino Sans','Yu Gothic UI','Noto Sans JP','Meiryo',sans-serif";

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn n(v: f32) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").unwrap_or(&s).to_string()
}

/// 単独の `.svg` ファイル。
pub fn svg_file(d: &Design, labels: Labels) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&svg(d, labels, true));
    s
}

/// `<svg>` 要素。`background` が真なら白い背景を敷く。
pub fn svg(d: &Design, labels: Labels, background: bool) -> String {
    let g = geometry(d, labels);
    let (w, h) = (g.size.0 + 2.0 * MARGIN, g.size.1 + 2.0 * MARGIN);
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\" font-family=\"{FONT}\" font-size=\"12\">\n",
        n(w.max(1.0)),
        n(h.max(1.0)),
        n(w.max(1.0)),
        n(h.max(1.0))
    );
    if background {
        let _ = writeln!(s, "<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>");
    }
    let _ = writeln!(s, "<g transform=\"translate({MARGIN},{MARGIN})\">");
    for e in &g.edges {
        let pts: Vec<String> = e
            .points
            .iter()
            .map(|p| format!("{},{}", n(p.0), n(p.1)))
            .collect();
        let t = &d.tables[e.child];
        let fk = &t.foreign_keys[e.fk];
        let _ = writeln!(
            s,
            "<g class=\"edge\" stroke=\"#64748b\" stroke-width=\"1.4\" fill=\"none\"><title>{}: {}.{} → {}</title><polyline points=\"{}\"/>",
            esc(&t.fk_name(fk)),
            esc(&t.name),
            esc(&fk.columns.join(",")),
            esc(&fk.ref_table),
            pts.join(" ")
        );
        let ends = [
            (e.child_marker, e.points[0], e.child_dir),
            (
                e.parent_marker,
                *e.points.last().unwrap_or(&e.points[0]),
                e.parent_dir,
            ),
        ];
        for (m, (x, y), dir) in ends {
            for shape in marker_shapes(m, x, y, dir) {
                match shape {
                    Shape::Line(x1, y1, x2, y2) => {
                        let _ = writeln!(
                            s,
                            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
                            n(x1),
                            n(y1),
                            n(x2),
                            n(y2)
                        );
                    }
                    Shape::Circle(cx, cy, r) => {
                        let _ = writeln!(
                            s,
                            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#ffffff\"/>",
                            n(cx),
                            n(cy),
                            n(r)
                        );
                    }
                }
            }
        }
        s.push_str("</g>\n");
    }
    for (t, r) in d.tables.iter().zip(&g.rects) {
        table_box(&mut s, t, *r, labels);
    }
    s.push_str("</g>\n</svg>\n");
    s
}

fn table_box(s: &mut String, t: &crate::model::Table, r: crate::diagram::Rect, labels: Labels) {
    let tip = [t.display_name(), &t.comment]
        .iter()
        .filter(|x| !x.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" / ");
    // 表の大きさ(`k`)は、文字の大きさではなく、箱の幅と見出し・行の高さ(=余白)に効く
    let k = t.factor();
    let (head_h, row_h) = (HEADER_H, row_height(k));
    let _ = writeln!(
        s,
        "<g class=\"table\" transform=\"translate({},{})\"><title>{}</title>",
        n(r.x),
        n(r.y),
        esc(&tip)
    );
    let _ = writeln!(
        s,
        "<rect width=\"{}\" height=\"{}\" rx=\"6\" fill=\"#ffffff\" stroke=\"#94a3b8\" stroke-width=\"1.2\"/>",
        n(r.w.max(MIN_WIDTH)),
        n(r.h)
    );
    let _ = writeln!(
        s,
        "<path d=\"M0,{h} V6 a6,6 0 0 1 6,-6 H{w6} a6,6 0 0 1 6,6 V{h} Z\" fill=\"#dbeafe\"/>",
        h = n(head_h),
        w6 = n(r.w - 6.0)
    );
    let (main, sub) = header_label(t, labels);
    let _ = writeln!(
        s,
        "<text x=\"{}\" y=\"{}\" font-size=\"13\" font-weight=\"bold\" fill=\"#0f172a\">{}</text>",
        n(PAD),
        n(head_h / 2.0 + 5.0),
        esc(&main)
    );
    if !sub.is_empty() {
        let _ = writeln!(
            s,
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"#475569\">{}</text>",
            n(PAD + text_width(&main) * HEADER_SCALE + 8.0),
            n(head_h / 2.0 + 5.0),
            esc(&sub)
        );
    }
    let _ = writeln!(
        s,
        "<line x1=\"0\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"#94a3b8\"/>",
        n(r.w),
        y = n(head_h)
    );
    for (i, c) in t.columns.iter().enumerate() {
        let y = row_center(i, k);
        let (name, ty) = column_label(c, labels);
        let badge = match (c.primary, t.is_foreign(&c.name)) {
            (true, true) => "PK,FK",
            (true, false) => "PK",
            (false, true) => "FK",
            _ => "",
        };
        if !badge.is_empty() {
            let _ = writeln!(
                s,
                "<text x=\"{}\" y=\"{}\" font-size=\"9\" fill=\"#64748b\">{badge}</text>",
                n(6.0),
                n(y + 3.0)
            );
        }
        let weight = if c.primary {
            " font-weight=\"bold\""
        } else {
            ""
        };
        let fill = if c.nullable && !c.primary {
            "#334155"
        } else {
            "#0f172a"
        };
        let _ = writeln!(
            s,
            "<text x=\"{}\" y=\"{}\"{weight} fill=\"{fill}\">{}</text>",
            n(BADGE_W),
            n(y + 4.0),
            esc(&name)
        );
        if !ty.is_empty() {
            let _ = writeln!(
                s,
                "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-size=\"11\" fill=\"#64748b\">{}</text>",
                n(r.w - PAD),
                n(y + 4.0),
                esc(&ty)
            );
        }
        if i + 1 < t.columns.len() {
            let ly = head_h + row_h * (i as f32 + 1.0);
            let _ = writeln!(
                s,
                "<line x1=\"0\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"#eef2f7\"/>",
                n(r.w),
                y = n(ly)
            );
        }
    }
    s.push_str("</g>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse;

    #[test]
    fn the_svg_has_every_table_column_and_relation() {
        let d = parse("a \"顧客\" \"メモ\"\n  id int pk\n  name text \"氏名\"\nb\n  id int pk\n  a_id int -> a.id\n").unwrap();
        let svg = svg_file(&d, Labels::Physical);
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert_eq!(svg.matches("class=\"table\"").count(), 2);
        assert_eq!(svg.matches("<polyline").count(), 1);
        for needle in [
            "顧客",
            ">name<",
            ">text<",
            "顧客 / メモ",
            "fk_b_a_id: b.a_id → a",
            ">FK<",
            ">PK<",
        ] {
            assert!(svg.contains(needle), "{needle}\n{svg}");
        }
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn the_logical_view_shows_logical_names_without_types() {
        let d = parse("a \"顧客\"\n  name text \"氏名\"\n").unwrap();
        let svg = svg(&d, Labels::Logical, false);
        assert!(svg.contains(">氏名<") && !svg.contains(">name<") && !svg.contains(">text<"));
        assert!(
            !svg.contains("fill=\"#ffffff\"/>\n<g transform"),
            "no background when embedded"
        );
    }

    #[test]
    fn a_table_scale_adds_room_without_enlarging_the_text() {
        let plain = svg(
            &parse("a @0,0\n  id int pk\n").unwrap(),
            Labels::Physical,
            false,
        );
        let big = svg(
            &parse("a @0,0,2\n  id int pk\n").unwrap(),
            Labels::Physical,
            false,
        );
        for s in [&plain, &big] {
            assert!(
                s.contains("font-size=\"13\"") && !s.contains("scale("),
                "{s}"
            );
        }
        // 見出しの高さは変わらず、列の行だけが高くなる
        assert!(
            plain.contains("M0,30 V6") && big.contains("M0,30 V6"),
            "{big}"
        );
        assert!(big.contains("height=\"80\""), "{big}");
    }

    #[test]
    fn text_is_escaped() {
        let d = parse("t \"<b>&\"\n  a int \"\\\"q\\\"\"\n").unwrap();
        let svg = svg(&d, Labels::Physical, false);
        assert!(svg.contains("&lt;b&gt;&amp;"));
        assert!(!svg.contains("<b>"));
        assert!(
            svg.contains("&quot;q&quot;") || !svg.contains("\"q\"<"),
            "{svg}"
        );
    }

    #[test]
    fn an_empty_design_is_still_a_valid_svg() {
        let svg = svg_file(&Design::default(), Labels::Physical);
        assert!(svg.contains("viewBox=\"0 0 48 48\""), "{svg}");
    }
}
