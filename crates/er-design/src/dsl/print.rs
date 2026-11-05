//! [`Design`] をテキスト記法にする。[`super::parse`] で読み戻すと同じ設計になる。

use std::fmt::Write as _;

use crate::model::{Column, Design, ForeignKey, RefAction, Table};

/// 表示上の幅(全角は 2)。桁をそろえるために使う。
fn width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 既定値。空白を含む式は、バッククォートで囲む(`'...'` の文字列はそのまま)。
fn default_value(v: &str) -> String {
    let literal = v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'');
    if literal || !v.contains(char::is_whitespace) && !v.is_empty() {
        v.to_string()
    } else {
        format!("`{v}`")
    }
}

fn actions(fk: &ForeignKey) -> String {
    let mut s = String::new();
    if fk.on_delete != RefAction::NoAction {
        let _ = write!(s, " on delete {}", fk.on_delete.sql().to_lowercase());
    }
    if fk.on_update != RefAction::NoAction {
        let _ = write!(s, " on update {}", fk.on_update.sql().to_lowercase());
    }
    s
}

fn target(fk: &ForeignKey, single: bool) -> String {
    match (single, fk.ref_columns.as_slice()) {
        (_, []) => fk.ref_table.clone(),
        (true, [c]) => format!("{}.{c}", fk.ref_table),
        (_, cols) => format!("{}({})", fk.ref_table, cols.join(", ")),
    }
}

fn column_line(c: &Column, fk: Option<&ForeignKey>, name_w: usize, type_w: usize) -> String {
    let mut s = format!(
        "  {} {}",
        pad(&c.name, name_w),
        pad(&c.ty.to_string(), type_w)
    );
    if c.primary {
        s.push_str(" pk");
    }
    if c.auto {
        s.push_str(" auto");
    }
    if !c.nullable && !c.primary {
        s.push_str(" not null");
    }
    if c.unique {
        s.push_str(" unique");
    }
    if let Some(d) = &c.default {
        let _ = write!(s, " default {}", default_value(d));
    }
    if let Some(fk) = fk {
        let _ = write!(s, " -> {}{}", target(fk, true), actions(fk));
    }
    if !c.logical.is_empty() || !c.comment.is_empty() {
        let _ = write!(s, " {}", quote(&c.logical));
    }
    if !c.comment.is_empty() {
        let _ = write!(s, " {}", quote(&c.comment));
    }
    s
}

/// 列の行に書ける外部キー(1 列で、名前の指定がないもの。同じ列に複数あれば最初の 1 本)の添字。
fn inline_fk(t: &Table, column: &str) -> Option<usize> {
    t.foreign_keys
        .iter()
        .position(|f| f.name.is_empty() && f.columns.len() == 1 && f.columns[0] == column)
}

fn named(prefix: &str, name: &str, cols: &[String]) -> String {
    let name = if name.is_empty() {
        String::new()
    } else {
        format!("{name} ")
    };
    format!("  {prefix} {name}({})", cols.join(", "))
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn print_table(out: &mut String, t: &Table) {
    out.push_str(&t.name);
    if !t.logical.is_empty() || !t.comment.is_empty() {
        let _ = write!(out, " {}", quote(&t.logical));
    }
    if !t.comment.is_empty() {
        let _ = write!(out, " {}", quote(&t.comment));
    }
    let scale = t.scale.filter(|s| (s - 1.0).abs() > 0.005);
    match (t.pos, scale) {
        (Some((x, y)), None) => {
            let _ = write!(out, " @{},{}", x.round(), y.round());
        }
        (Some((x, y)), Some(s)) => {
            let _ = write!(out, " @{},{},{}", x.round(), y.round(), round2(s));
        }
        (None, Some(s)) => {
            let _ = write!(out, " @auto,{}", round2(s));
        }
        (None, None) => {}
    }
    out.push('\n');
    let name_w = t.columns.iter().map(|c| width(&c.name)).max().unwrap_or(0);
    let type_w = t
        .columns
        .iter()
        .map(|c| c.ty.to_string().len())
        .max()
        .unwrap_or(0);
    for c in &t.columns {
        let fk = inline_fk(t, &c.name).map(|i| &t.foreign_keys[i]);
        out.push_str(&column_line(c, fk, name_w, type_w));
        out.push('\n');
    }
    for u in &t.uniques {
        let _ = writeln!(out, "{}", named("unique", &u.name, &u.columns));
    }
    for i in &t.indexes {
        let unique = if i.unique { " unique" } else { "" };
        let _ = writeln!(out, "{}{unique}", named("index", &i.name, &i.columns));
    }
    let inline: Vec<usize> = t
        .columns
        .iter()
        .filter_map(|c| inline_fk(t, &c.name))
        .collect();
    for (_, f) in t
        .foreign_keys
        .iter()
        .enumerate()
        .filter(|(i, _)| !inline.contains(i))
    {
        let head = named("fk", &f.name, &f.columns);
        let _ = writeln!(out, "{head} -> {}{}", target(f, false), actions(f));
    }
}

pub fn print(d: &Design) -> String {
    let mut out = String::new();
    for (key, value) in [
        ("@title", &d.title),
        ("@description", &d.description),
        ("@author", &d.author),
        ("@version", &d.version),
    ] {
        if !value.is_empty() {
            let _ = writeln!(out, "{key} {}", quote(value));
        }
    }
    for t in &d.tables {
        if !out.is_empty() {
            out.push('\n');
        }
        print_table(&mut out, t);
    }
    out
}
