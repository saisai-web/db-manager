//! 設計書の HTML。表紙・目次・表の一覧・ER 図・表ごとの定義・関係の一覧(・CREATE 文)を、1 ファイルにする。

use std::fmt::Write as _;

use super::css::CSS;
use super::rows::{column_rows, constraint_rows, referenced_by, references, relation_rows};
use super::{DocOptions, doc_title};
use crate::ddl::{DdlOptions, generate};
use crate::model::{Design, Table};
use crate::svg::{esc, svg};

/// 改行を `<br>` にして、エスケープする。
fn text(s: &str) -> String {
    esc(s).replace('\n', "<br>")
}

fn mark(on: bool) -> &'static str {
    if on { "○" } else { "" }
}

fn anchor(d: &Design, name: &str) -> String {
    d.table_index(name).map_or_else(
        || esc(name),
        |i| format!("<a href=\"#t{i}\">{}</a>", esc(name)),
    )
}

pub fn html(d: &Design, opts: &DocOptions) -> String {
    let title = doc_title(d);
    let mut s = String::new();
    let _ = write!(
        s,
        "<!DOCTYPE html>\n<html lang=\"ja\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n<div class=\"page\">\n",
        esc(title)
    );
    cover(&mut s, d, opts);
    toc(&mut s, opts);
    table_list(&mut s, d);
    let _ = write!(
        s,
        "<h2 id=\"diagram\">ER 図</h2>\n<div class=\"diagram\">\n{}</div>\n",
        svg(d, opts.labels, false)
    );
    s.push_str("<h2 id=\"definitions\">テーブル定義</h2>\n");
    for (i, t) in d.tables.iter().enumerate() {
        table_section(&mut s, d, t, i);
    }
    relations(&mut s, d);
    if let Some(target) = opts.ddl {
        let ddl = generate(d, target, &DdlOptions::default());
        let _ = write!(
            s,
            "<h2 id=\"ddl\">付録: CREATE 文({})</h2>\n<pre class=\"sql\">{}</pre>\n",
            target.label(),
            esc(&ddl.sql)
        );
    }
    s.push_str("<footer>SQLツールで作成</footer>\n</div>\n</body>\n</html>\n");
    s
}

fn cover(s: &mut String, d: &Design, opts: &DocOptions) {
    let _ = write!(
        s,
        "<header class=\"cover\">\n<h1>{}</h1>\n<div class=\"meta\">",
        esc(doc_title(d))
    );
    for (label, value) in [
        ("バージョン", d.version.as_str()),
        ("作成者", d.author.as_str()),
        ("作成日", opts.date.as_str()),
    ] {
        if !value.is_empty() {
            let _ = write!(s, "<span>{label}: <b>{}</b></span>", esc(value));
        }
    }
    let relations = d.tables.iter().map(|t| t.foreign_keys.len()).sum::<usize>();
    let _ = write!(
        s,
        "<span>表: <b>{}</b></span><span>外部キー: <b>{relations}</b></span></div>\n</header>\n",
        d.tables.len()
    );
    if !d.description.is_empty() {
        let _ = write!(
            s,
            "<h2>概要</h2>\n<p class=\"desc\">{}</p>\n",
            esc(&d.description)
        );
    }
}

fn toc(s: &mut String, opts: &DocOptions) {
    s.push_str("<nav class=\"toc\">\n<h2>目次</h2>\n<ol>\n<li><a href=\"#list\">テーブル一覧</a></li>\n<li><a href=\"#diagram\">ER 図</a></li>\n<li><a href=\"#definitions\">テーブル定義</a></li>\n<li><a href=\"#relations\">リレーション一覧</a></li>\n");
    if opts.ddl.is_some() {
        s.push_str("<li><a href=\"#ddl\">付録: CREATE 文</a></li>\n");
    }
    s.push_str("</ol>\n</nav>\n");
}

fn table_list(s: &mut String, d: &Design) {
    s.push_str("<h2 id=\"list\">テーブル一覧</h2>\n<table>\n<thead><tr><th>No</th><th>物理名</th><th>論理名</th><th>概要</th><th>列数</th><th>主キー</th></tr></thead>\n<tbody>\n");
    for (i, t) in d.tables.iter().enumerate() {
        let _ = writeln!(
            s,
            "<tr><td class=\"no\">{}</td><td class=\"mono\"><a href=\"#t{i}\">{}</a></td><td>{}</td><td>{}</td><td class=\"c\">{}</td><td class=\"mono\">{}</td></tr>",
            i + 1,
            esc(&t.name),
            esc(&t.logical),
            text(&t.comment),
            t.columns.len(),
            esc(&t.primary_key().join(", "))
        );
    }
    s.push_str("</tbody>\n</table>\n");
}

fn table_section(s: &mut String, d: &Design, t: &Table, i: usize) {
    let sub = if t.logical.is_empty() {
        String::new()
    } else {
        format!("<small>{}</small>", esc(&t.logical))
    };
    let _ = write!(
        s,
        "<section class=\"table\" id=\"t{i}\">\n<h3><span class=\"mono\">{}</span>{sub}</h3>\n",
        esc(&t.name)
    );
    if !t.comment.is_empty() {
        let _ = writeln!(s, "<p class=\"desc\">{}</p>", esc(&t.comment));
    }
    let links = |label: &str, names: Vec<String>| {
        if names.is_empty() {
            String::new()
        } else {
            let l: Vec<_> = names.iter().map(|n| anchor(d, n)).collect();
            format!("<span>{label}: {}</span> ", l.join(", "))
        }
    };
    let refs = format!(
        "{}{}",
        links("参照する表", references(d, t)),
        links("参照される表", referenced_by(d, t))
    );
    if !refs.is_empty() {
        let _ = writeln!(s, "<div class=\"refs\">{refs}</div>");
    }
    s.push_str("<table>\n<thead><tr><th>No</th><th>論理名</th><th>物理名</th><th>型</th><th>NOT NULL</th><th>PK</th><th>FK(参照先)</th><th>UQ</th><th>連番</th><th>既定値</th><th>備考</th></tr></thead>\n<tbody>\n");
    for r in column_rows(d, t) {
        let _ = writeln!(
            s,
            "<tr><td class=\"no\">{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"c\">{}</td><td class=\"c\">{}</td><td class=\"mono\">{}</td><td class=\"c\">{}</td><td class=\"c\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>",
            r.no,
            esc(&r.logical),
            esc(&r.name),
            esc(&r.ty),
            mark(r.not_null),
            mark(r.primary),
            esc(&r.foreign),
            mark(r.unique),
            mark(r.auto),
            esc(&r.default),
            text(&r.comment)
        );
    }
    s.push_str("</tbody>\n</table>\n");
    let cons = constraint_rows(d, t);
    if !cons.is_empty() {
        s.push_str("<h4>制約・索引</h4>\n<table>\n<thead><tr><th>種類</th><th>名前</th><th>列</th><th>参照先</th><th>削除時</th><th>更新時</th></tr></thead>\n<tbody>\n");
        for c in cons {
            let _ = writeln!(
                s,
                "<tr><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td></tr>",
                c.kind,
                esc(&c.name),
                esc(&c.columns),
                esc(&c.target),
                c.on_delete,
                c.on_update
            );
        }
        s.push_str("</tbody>\n</table>\n");
    }
    s.push_str("</section>\n");
}

fn relations(s: &mut String, d: &Design) {
    s.push_str("<h2 id=\"relations\">リレーション一覧</h2>\n");
    let rows = relation_rows(d);
    if rows.is_empty() {
        s.push_str("<p>外部キーによる関係はありません。</p>\n");
        return;
    }
    s.push_str("<table>\n<thead><tr><th>親の表</th><th>子の表</th><th>外部キー名</th><th>列</th><th>多重度(親 : 子)</th><th>削除時</th><th>更新時</th></tr></thead>\n<tbody>\n");
    for r in rows {
        let _ = writeln!(
            s,
            "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"c\">{}</td><td>{}</td><td>{}</td></tr>",
            anchor(d, &r.parent),
            anchor(d, &r.child),
            esc(&r.name),
            esc(&r.columns),
            r.cardinality,
            r.on_delete,
            r.on_update
        );
    }
    s.push_str("</tbody>\n</table>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddl::Target;
    use crate::dsl::parse;

    const SRC: &str = "@title ショップ\n@description \"行1\\n行2 <b>\"\n@author 佐伯\n@version 2.0\ncustomers \"顧客\" \"会員の\\n基本情報\"\n  id int pk auto \"ID\"\n  email varchar(255) unique default 'x' \"メール\" \"連絡<用>\"\norders\n  id int pk\n  customer_id int not null -> customers.id on delete cascade\n  index (customer_id)\n";

    #[test]
    fn the_document_has_every_section_and_links_between_them() {
        let d = parse(SRC).unwrap();
        let h = html(
            &d,
            &DocOptions {
                date: "2026-11-04".into(),
                ddl: Some(Target::Postgres),
                ..Default::default()
            },
        );
        for needle in [
            "<title>ショップ</title>",
            "バージョン: <b>2.0</b>",
            "作成日: <b>2026-11-04</b>",
            "id=\"list\"",
            "id=\"diagram\"",
            "id=\"definitions\"",
            "id=\"relations\"",
            "id=\"ddl\"",
            "<svg ",
            "id=\"t0\"",
            "id=\"t1\"",
            "<a href=\"#t0\">customers</a>",
            "参照される表: <a href=\"#t1\">orders</a>",
            "fk_orders_customer_id",
            "連鎖(CASCADE)",
            "1 : 0..N",
            "CREATE TABLE customers",
            "付録: CREATE 文(PostgreSQL)",
        ] {
            assert!(h.contains(needle), "missing {needle}");
        }
        assert!(h.trim_end().ends_with("</html>"));
    }

    #[test]
    fn user_text_is_escaped_and_newlines_become_breaks() {
        let d = parse(SRC).unwrap();
        let h = html(&d, &DocOptions::default());
        assert!(
            h.contains("行1\n行2 &lt;b&gt;"),
            "descriptions keep their newlines (pre-wrap)"
        );
        assert!(h.contains("会員の<br>基本情報"));
        assert!(h.contains("連絡&lt;用&gt;"));
        assert!(!h.contains("<b>行") && !h.contains("連絡<用>"));
    }

    #[test]
    fn optional_parts_are_left_out() {
        let d = parse("t\n  id int pk\n").unwrap();
        let h = html(&d, &DocOptions::default());
        assert!(!h.contains("id=\"ddl\"") && !h.contains("作成日"));
        assert!(h.contains("<title>データベース設計書</title>"));
        assert!(h.contains("外部キーによる関係はありません"));
    }

    #[test]
    fn an_empty_design_renders() {
        let h = html(&Design::default(), &DocOptions::default());
        assert!(h.contains("表: <b>0</b>"));
    }
}
