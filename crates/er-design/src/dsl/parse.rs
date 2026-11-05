//! テキスト記法を読み、[`Design`] にする。誤りは行ごとにすべて集めて返す。

use super::lex::{Tok, split_list, tokenize};
use super::{ParseError, is_valid_name};
use crate::model::{Column, Design, ForeignKey, Index, RefAction, Table, UniqueKey};
use crate::types::DataType;

type R<T> = Result<T, String>;

/// 表の中の `pk (a, b)`(列が全部そろってから反映する)
struct PendingPk {
    line: usize,
    columns: Vec<String>,
}

#[derive(Default)]
struct Builder {
    design: Design,
    errors: Vec<ParseError>,
    current: Option<(Table, Vec<PendingPk>)>,
}

impl Builder {
    fn error(&mut self, line: usize, message: impl Into<String>) {
        self.errors.push(ParseError {
            line,
            message: message.into(),
        });
    }

    fn finish_table(&mut self, line: usize) {
        let Some((mut table, pks)) = self.current.take() else {
            return;
        };
        for pk in pks {
            for name in &pk.columns {
                match table.columns.iter_mut().find(|c| &c.name == name) {
                    Some(c) => {
                        c.primary = true;
                        c.nullable = false;
                    }
                    None => self.error(pk.line, format!("主キーの列 '{name}' がありません")),
                }
            }
        }
        if self.design.table(&table.name).is_some() {
            self.error(line, format!("表 '{}' が重複しています", table.name));
        }
        self.design.tables.push(table);
    }
}

pub fn parse(src: &str) -> Result<Design, Vec<ParseError>> {
    let mut b = Builder::default();
    let mut last = 0;
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        last = line;
        let text = raw.trim_end();
        let body = text.trim_start();
        if body.is_empty() || body.starts_with('#') || body.starts_with("//") {
            continue;
        }
        let result = if text.starts_with([' ', '\t']) {
            member(&mut b, line, body)
        } else if body.starts_with('@') {
            b.finish_table(line);
            meta(&mut b.design, body)
        } else {
            b.finish_table(line);
            header(&mut b, body)
        };
        if let Err(message) = result {
            b.error(line, message);
        }
    }
    b.finish_table(last);
    if b.errors.is_empty() {
        Ok(b.design)
    } else {
        Err(b.errors)
    }
}

fn meta(d: &mut Design, body: &str) -> R<()> {
    let (key, rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
    let rest = rest.trim();
    let value = match tokenize(rest)?.as_slice() {
        [Tok::Quoted(s)] => s.clone(),
        _ => rest.to_string(),
    };
    match key {
        "@title" => d.title = value,
        "@description" => d.description = value,
        "@author" => d.author = value,
        "@version" => d.version = value,
        _ => {
            return Err(format!(
                "不明な設定 '{key}'(@title @description @author @version)"
            ));
        }
    }
    Ok(())
}

/// `@120,80` の座標。`@120,80,1.5` なら、箱の拡大率も。座標を自動配置のままにするなら `@auto,1.5`。
/// 表の行の `@...` の指定: (位置, 拡大率)。
type Placement = (Option<(f32, f32)>, Option<f32>);

fn position(word: &str) -> R<Placement> {
    let bad = || format!("位置の書き方が違います: '{word}'(例: @120,80 または @120,80,1.5)");
    let parts: Vec<&str> = word[1..].split(',').map(str::trim).collect();
    let num = |s: &str| s.parse::<f32>().map_err(|_| bad());
    let scale = |s: Option<&&str>| s.map(|s| num(s)).transpose();
    match parts.as_slice() {
        ["auto", s] => Ok((None, Some(num(s)?))),
        [x, y] | [x, y, _] => Ok((Some((num(x)?, num(y)?)), scale(parts.get(2))?)),
        _ => Err(bad()),
    }
}

fn check_name(name: &str, what: &str) -> R<()> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(format!(
            "{what}の名前 '{name}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
        ))
    }
}

/// 論理名と備考(`"..."` の 1 つ目と 2 つ目)。
fn two_strings(strings: Vec<String>, what: &str) -> R<(String, String)> {
    if strings.len() > 2 {
        return Err(format!("{what}の \"...\" は、論理名と備考の 2 つまでです"));
    }
    let mut s = strings.into_iter();
    Ok((s.next().unwrap_or_default(), s.next().unwrap_or_default()))
}

fn header(b: &mut Builder, body: &str) -> R<()> {
    let toks = tokenize(body)?;
    let mut it = toks.into_iter();
    let Some(Tok::Word(name)) = it.next() else {
        return Err("表の名前から書き始めてください".into());
    };
    check_name(&name, "表")?;
    let mut table = Table::new(name);
    let mut strings = Vec::new();
    for t in it {
        match t {
            Tok::Quoted(s) => strings.push(s),
            Tok::Word(w) if w.starts_with('@') => {
                (table.pos, table.scale) = position(&w)?;
            }
            other => return Err(format!("表の行に書けない指定があります: {other:?}")),
        }
    }
    (table.logical, table.comment) = two_strings(strings, "表の行")?;
    b.current = Some((table, Vec::new()));
    Ok(())
}

fn member(b: &mut Builder, line: usize, body: &str) -> R<()> {
    let toks = tokenize(body)?;
    let Some((table, pks)) = b.current.as_mut() else {
        return Err("列は、表の行の下にインデントして書きます".into());
    };
    if let Some(kind) = table_level_keyword(&toks) {
        return table_level(table, pks, line, kind, &toks[1..]);
    }
    let (column, fk) = column(&toks)?;
    if table.column(&column.name).is_some() {
        return Err(format!("列 '{}' が重複しています", column.name));
    }
    table.columns.push(column);
    table.foreign_keys.extend(fk);
    Ok(())
}

/// 表の設定の行(`pk (...)`、`unique (...)`、`index (...)`、`fk (...) -> ...`)か。
/// `index int` のような列の行と区別するため、括弧の列が直後か、名前をはさんだ次にあるものだけ。
fn table_level_keyword(toks: &[Tok]) -> Option<&'static str> {
    let Some(Tok::Word(w)) = toks.first() else {
        return None;
    };
    let kw = ["pk", "unique", "index", "fk"]
        .into_iter()
        .find(|k| w == k)?;
    let paren_at = |i: usize| matches!(toks.get(i), Some(Tok::Paren(_)));
    let named = matches!(toks.get(1), Some(Tok::Word(_))) && paren_at(2);
    (paren_at(1) || named).then_some(kw)
}

fn column(toks: &[Tok]) -> R<(Column, Option<ForeignKey>)> {
    let Some(Tok::Word(name)) = toks.first() else {
        return Err("列の名前から書き始めてください".into());
    };
    let name = name.trim_end_matches(':');
    check_name(name, "列")?;
    let Some(Tok::Word(ty)) = toks.get(1) else {
        return Err(format!("列 '{name}' の型がありません(例: {name} int)"));
    };
    let mut c = Column::new(name, DataType::parse(ty));
    let mut fk = None;
    let mut strings = Vec::new();
    let word_is =
        |i: usize, w: &str| matches!(toks.get(i), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w));
    let mut i = 2;
    while i < toks.len() {
        i += 1;
        match &toks[i - 1] {
            Tok::Quoted(s) => strings.push(s.clone()),
            Tok::Word(w) => match w.to_ascii_lowercase().as_str() {
                "pk" | "primary" => {
                    c.primary = true;
                    if word_is(i, "key") {
                        i += 1;
                    }
                }
                "not" if word_is(i, "null") => {
                    c.nullable = false;
                    i += 1;
                }
                "!" | "nn" | "notnull" => c.nullable = false,
                "null" => c.nullable = true,
                "unique" | "uq" => c.unique = true,
                "auto" | "autoincrement" | "auto_increment" | "identity" | "serial" => {
                    c.auto = true
                }
                "default" => {
                    c.default = Some(match toks.get(i) {
                        Some(Tok::Word(v) | Tok::Lit(v) | Tok::Raw(v)) => v.clone(),
                        Some(Tok::Quoted(v)) => format!("'{}'", v.replace('\'', "''")),
                        _ => return Err("'default' のあとに値がありません".into()),
                    });
                    i += 1;
                }
                "->" => {
                    let Some(Tok::Word(target)) = toks.get(i) else {
                        return Err("'->' のあとに参照先がありません(例: -> customers.id)".into());
                    };
                    let (ref_table, ref_columns) = reference(target)?;
                    if ref_columns.len() > 1 {
                        return Err(
                            "列の行の '->' で参照できるのは 1 列です(複数列は fk (...) を使う)"
                                .into(),
                        );
                    }
                    let mut f = ForeignKey {
                        columns: vec![c.name.clone()],
                        ref_table,
                        ref_columns,
                        ..Default::default()
                    };
                    i = actions(toks, i + 1, &mut f)?;
                    fk = Some(f);
                }
                other => return Err(format!("不明な指定 '{other}'")),
            },
            other => return Err(format!("ここに書けない指定があります: {other:?}")),
        }
    }
    (c.logical, c.comment) = two_strings(strings, "列の行")?;
    if c.primary {
        c.nullable = false;
    }
    Ok((c, fk))
}

/// `t.c`、`t(c)`、`t(a, b)`、`t` を (表, 列) にする。
fn reference(word: &str) -> R<(String, Vec<String>)> {
    let (table, columns) = if let Some(open) = word.find('(') {
        let inner = word[open + 1..]
            .strip_suffix(')')
            .ok_or_else(|| format!("参照先の書き方が違います: '{word}'"))?;
        (&word[..open], split_list(inner))
    } else if let Some((t, c)) = word.split_once('.') {
        (t, vec![c.to_string()])
    } else {
        (word, Vec::new())
    };
    check_name(table, "参照先の表")?;
    for c in &columns {
        check_name(c, "参照先の列")?;
    }
    Ok((table.to_string(), columns))
}

/// `on delete cascade` / `on update set null` を、`start` から読む。続く `"..."` の手前で止まり、次の位置を返す。
fn actions(toks: &[Tok], start: usize, fk: &mut ForeignKey) -> R<usize> {
    let word = |i: usize| match toks.get(i) {
        Some(Tok::Word(w)) => Some(w.to_ascii_lowercase()),
        _ => None,
    };
    let mut i = start;
    while word(i).as_deref() == Some("on") {
        let which = word(i + 1).ok_or("'on' のあとは delete か update です")?;
        let (action, used) = match (word(i + 2).as_deref(), word(i + 3).as_deref()) {
            (Some("cascade"), _) => (RefAction::Cascade, 1),
            (Some("restrict"), _) => (RefAction::Restrict, 1),
            (Some("set"), Some("null")) => (RefAction::SetNull, 2),
            (Some("set"), Some("default")) => (RefAction::SetDefault, 2),
            (Some("no"), Some("action")) => (RefAction::NoAction, 2),
            _ => {
                return Err(
                    "動作は cascade / restrict / set null / set default / no action です".into(),
                );
            }
        };
        match which.as_str() {
            "delete" => fk.on_delete = action,
            "update" => fk.on_update = action,
            _ => return Err("'on' のあとは delete か update です".into()),
        }
        i += 2 + used;
    }
    Ok(i)
}

fn list_of(toks: &[Tok], from: usize) -> R<Vec<String>> {
    match toks.get(from) {
        Some(Tok::Paren(cols)) if !cols.is_empty() => Ok(cols.clone()),
        _ => Err("列を ( ) で囲んで書いてください".into()),
    }
}

fn table_level(
    table: &mut Table,
    pks: &mut Vec<PendingPk>,
    line: usize,
    kind: &str,
    toks: &[Tok],
) -> R<()> {
    // 名前(省略可)のあとに ( 列 )
    let (name, at) = match toks.first() {
        Some(Tok::Word(n)) => (n.clone(), 1),
        _ => (String::new(), 0),
    };
    let columns = list_of(toks, at)?;
    let after = at + 1;
    match kind {
        "pk" => pks.push(PendingPk { line, columns }),
        "unique" => table.uniques.push(UniqueKey { name, columns }),
        "index" => {
            let unique =
                matches!(toks.get(after), Some(Tok::Word(w)) if w.eq_ignore_ascii_case("unique"));
            if toks.len() > after + usize::from(unique) {
                return Err("index の行の最後に余分な指定があります".into());
            }
            table.indexes.push(Index {
                name,
                columns,
                unique,
            });
        }
        _ => {
            if !matches!(toks.get(after), Some(Tok::Word(w)) if w == "->") {
                return Err("fk (列) -> 表(列) の形で書いてください".into());
            }
            let Some(Tok::Word(target)) = toks.get(after + 1) else {
                return Err("'->' のあとに参照先がありません".into());
            };
            let (ref_table, mut ref_columns) = reference(target)?;
            let mut next = after + 2;
            if let Some(Tok::Paren(cols)) = toks.get(next) {
                ref_columns = cols.clone();
                next += 1;
            }
            let mut fk = ForeignKey {
                name,
                columns,
                ref_table,
                ref_columns,
                ..Default::default()
            };
            if actions(toks, next, &mut fk)? != toks.len() {
                return Err("fk の行の最後に余分な指定があります".into());
            }
            table.foreign_keys.push(fk);
        }
    }
    Ok(())
}
