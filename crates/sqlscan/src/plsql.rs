//! Oracle の PL/SQL ブロックの、終わりの見つけ方。
//!
//! `BEGIN ... END;` や `CREATE PROCEDURE ... END;` の内側の `;` は文の区切りではない。
//! PL/SQL を解析せず、`BEGIN` / `CASE` の数と `END` の数が釣り合う所を、ブロックの終わりとする。
//! 判断できない形(`COMPOUND TRIGGER` など)は `;` では分割せず、単独行の `/` に任せる。

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Unit {
    /// PL/SQL ではない(普通の SQL 文)
    #[default]
    None,
    /// `DECLARE` / `BEGIN` で始まるブロック、`CREATE PROCEDURE|FUNCTION|TRIGGER`
    Block,
    /// `CREATE PACKAGE [BODY]`、`CREATE TYPE BODY`。最初の `IS` / `AS` から最後の `END name;` まで
    Package,
    /// 形を判断できない。`;` では終わらない
    Opaque,
    /// PL/SQL の見出しで始まるが本体を持たない(`AS LANGUAGE JAVA NAME '...'`)。普通の文として `;` で終わる
    Plain,
}

#[derive(Default)]
pub(crate) struct Block {
    unit: Unit,
    /// 文頭から見た語(`CREATE OR REPLACE EDITIONABLE` を読み飛ばすため、先頭の数語だけ)
    head: Vec<String>,
    /// 本体を開始した(`BEGIN`、またはパッケージの `IS` / `AS`)
    started: bool,
    /// 対応する `END` を待つ `BEGIN` / `CASE` の数
    depth: u32,
    /// 直前の語が `END`
    after_end: bool,
    /// 入れ子のサブプログラムの見出し(`PROCEDURE p` / `FUNCTION f`)を読んでいる途中
    in_sub_header: bool,
    /// `IS` / `AS` まで読んだ、本体(`BEGIN`)をまだ待つ入れ子のサブプログラムの数
    pending_bodies: u32,
    /// 本体を持つ単位の、自身の `IS` / `AS` を読み終えた
    own_is_seen: bool,
}

impl Block {
    /// 括弧の外・内を問わず、文の語を順に渡す(大文字にしたもの)。
    pub fn word(&mut self, w: &str) {
        if self.head.len() < 6 {
            self.head.push(w.to_string());
        }
        if self.unit == Unit::None {
            self.detect();
            // 単位を決めた語そのもの(`PROCEDURE` など)は、中身として数えない。`BEGIN` だけは本体の始まり
            if self.unit == Unit::None || self.head[0] != "BEGIN" {
                return;
            }
        }
        if matches!(self.unit, Unit::None | Unit::Opaque | Unit::Plain) {
            return;
        }
        if self.after_end {
            self.after_end = false;
            match w {
                "IF" | "LOOP" => return,
                "CASE" => {
                    self.close();
                    return;
                }
                _ => self.close(), // `END;` と `END name;`
            }
        }
        match w {
            "END" => self.after_end = true,
            "CASE" => self.depth += 1,
            "BEGIN" => self.begin(),
            "PROCEDURE" | "FUNCTION" if self.unit == Unit::Block && !self.started => {
                self.in_sub_header = true;
            }
            "PROCEDURE" | "FUNCTION" if self.unit == Unit::Package && self.started => {
                self.in_sub_header = true;
            }
            "IS" | "AS" => self.is_or_as(),
            "COMPOUND" => self.unit = Unit::Opaque,
            "LANGUAGE" | "EXTERNAL" if !self.started => self.unit = Unit::Plain,
            _ => {}
        }
    }

    /// `;` を渡す。見出しだけの宣言(`PROCEDURE p;`)を終わらせ、`END` を確定する。
    pub fn semicolon(&mut self) {
        if matches!(self.unit, Unit::None | Unit::Opaque | Unit::Plain) {
            return;
        }
        if self.after_end {
            self.after_end = false;
            self.close();
        }
        self.in_sub_header = false;
    }

    /// ここに `;` が来たら、文が終わるか。
    pub fn is_complete(&self) -> bool {
        match self.unit {
            Unit::None | Unit::Plain => true,
            Unit::Opaque => false,
            Unit::Block | Unit::Package => self.started && self.depth == 0,
        }
    }

    /// PL/SQL の単位として扱っている(`;` を文の一部に含める)。
    pub fn is_unit(&self) -> bool {
        matches!(self.unit, Unit::Block | Unit::Package)
    }

    fn detect(&mut self) {
        let h = &self.head;
        let word = |i: usize| h.get(i).map_or("", String::as_str);
        match word(0) {
            "DECLARE" | "BEGIN" => self.unit = Unit::Block,
            "CREATE" => {
                // `OR REPLACE`、`EDITIONABLE`、`NONEDITIONABLE` を読み飛ばす
                let mut i = 1;
                if word(i) == "OR" && word(i + 1) == "REPLACE" {
                    i += 2;
                }
                if matches!(word(i), "EDITIONABLE" | "NONEDITIONABLE") {
                    i += 1;
                }
                match (word(i), word(i + 1)) {
                    ("PROCEDURE" | "FUNCTION" | "TRIGGER", _) => self.unit = Unit::Block,
                    ("PACKAGE", _) | ("TYPE", "BODY") => self.unit = Unit::Package,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn begin(&mut self) {
        if self.pending_bodies > 0 {
            // 入れ子のサブプログラムの本体
            self.pending_bodies -= 1;
            self.depth += 1;
        } else if self.unit == Unit::Package && self.started && self.depth == 1 {
            // パッケージの初期化部の `BEGIN` は、パッケージの `END name;` と共有する
        } else {
            self.started = true;
            self.depth += 1;
        }
    }

    fn is_or_as(&mut self) {
        if self.in_sub_header {
            self.in_sub_header = false;
            self.pending_bodies += 1;
        } else if !self.own_is_seen && self.unit == Unit::Package {
            self.own_is_seen = true;
            self.started = true;
            self.depth += 1;
        } else {
            self.own_is_seen = true;
        }
    }

    fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::Block;

    /// 空白区切りの語を渡し、`;` の位置で文が終わるかを集める。
    fn ends(sql: &str) -> Vec<bool> {
        let mut b = Block::default();
        let mut out = Vec::new();
        for token in sql.split_whitespace() {
            let (word, semi) = match token.strip_suffix(';') {
                Some(w) => (w, true),
                None => (token, false),
            };
            if !word.is_empty() {
                b.word(&word.to_ascii_uppercase());
            }
            if semi {
                b.semicolon();
                out.push(b.is_complete());
            }
        }
        out
    }

    #[test]
    fn plain_sql_ends_at_the_first_semicolon() {
        assert_eq!(ends("select 1 from dual;"), [true]);
        assert_eq!(ends("create table t (a int);"), [true]);
    }

    #[test]
    fn anonymous_blocks_end_at_the_matching_end() {
        assert_eq!(ends("begin null; end;"), [false, true]);
        assert_eq!(
            ends("declare x number; begin x := 1; end;"),
            [false, false, true]
        );
        assert_eq!(
            ends("begin if a then b; end if; loop c; end loop; d; end;"),
            [false, false, false, false, false, true]
        );
    }

    #[test]
    fn case_statements_and_expressions_are_balanced() {
        assert_eq!(
            ends("begin x := case when a then 1 else 2 end; case when b then c; end case; end;"),
            [false, false, false, true]
        );
    }

    #[test]
    fn nested_blocks_and_labels() {
        assert_eq!(
            ends("begin begin null; end; null; end outer;"),
            [false, false, false, true]
        );
    }

    #[test]
    fn routines_end_at_the_last_end() {
        assert_eq!(
            ends("create or replace procedure p is v number; begin v := 1; end p;"),
            [false, false, true]
        );
        assert_eq!(
            ends("create function f return number as begin return 1; end;"),
            [false, true]
        );
    }

    #[test]
    fn nested_subprograms_in_declarations_do_not_end_the_block() {
        assert_eq!(
            ends("declare procedure q is begin null; end; begin q; end;"),
            [false, false, false, true]
        );
        assert_eq!(
            ends("declare procedure fwd; procedure q is begin null; end; begin q; end;"),
            [false, false, false, false, true]
        );
    }

    #[test]
    fn packages_end_at_the_closing_end_name() {
        assert_eq!(
            ends("create package p as procedure a; function b return number; end p;"),
            [false, false, true]
        );
        assert_eq!(
            ends("create package body p as procedure a is begin null; end; end p;"),
            [false, false, true]
        );
        // 初期化部の BEGIN は、パッケージの END を共有する
        assert_eq!(
            ends("create package body p as procedure a is begin null; end; begin init; end p;"),
            [false, false, false, true]
        );
    }

    #[test]
    fn unknown_shapes_are_left_to_the_slash_or_end_plainly() {
        assert_eq!(
            ends(
                "create trigger t for insert on x compound trigger before statement is begin null; end before statement; end t;"
            ),
            [false, false, false]
        );
        assert_eq!(
            ends("create function f return number as language java name 'x';"),
            [true]
        );
    }
}
