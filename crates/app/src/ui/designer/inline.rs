//! 図の上でのその場編集。表名・列名・型をクリックすると、その場が入力欄になる(Enter か、ほかをクリックで確定)。

use gpui_kit::component::input::{InputContent, InputEvent, InputState};
use sqltool_core::design::diagram::Labels;
use sqltool_core::design::{DataType, Design, columns, dsl};

use super::Designer;
use crate::ui::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Field {
    Name,
    Type,
}

pub(in crate::ui) struct Inline {
    pub table: usize,
    pub column: Option<usize>,
    pub field: Field,
    pub input: Entity<InputState>,
    _sub: Subscription,
}

impl Inline {
    pub(super) fn is(&self, table: usize, column: Option<usize>, field: Field) -> bool {
        (self.table, self.column, self.field) == (table, column, field)
    }
}

/// 入力欄に最初に入れる文字列。論理名の表示のときは、論理名を編集する。
fn initial(
    d: &Design,
    labels: Labels,
    table: usize,
    column: Option<usize>,
    field: Field,
) -> String {
    let Some(t) = d.tables.get(table) else {
        return String::new();
    };
    match (column.and_then(|c| t.columns.get(c)), field, labels) {
        (None, _, Labels::Physical) => t.name.clone(),
        (None, _, Labels::Logical) => t.logical.clone(),
        (Some(c), Field::Type, _) => c.ty.to_string(),
        (Some(c), Field::Name, Labels::Physical) => c.name.clone(),
        (Some(c), Field::Name, Labels::Logical) => c.logical.clone(),
    }
}

/// 確定した文字列を設計に反映する。
fn apply(
    d: &mut Design,
    labels: Labels,
    (table, column, field): (usize, Option<usize>, Field),
    value: &str,
) -> Result<String, String> {
    let table_name = d
        .tables
        .get(table)
        .map(|t| t.name.clone())
        .ok_or("表が見つかりません")?;
    let Some(ci) = column else {
        if labels == Labels::Logical {
            d.tables[table].logical = value.to_string();
            return Ok("表の論理名を変えました".into());
        }
        if value != table_name {
            if !dsl::is_valid_name(value) {
                return Err(format!(
                    "表の名前 '{value}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
                ));
            }
            if d.table(value).is_some() {
                return Err(format!("表 {value} は、すでにあります"));
            }
            d.rename_table(&table_name, value);
        }
        return Ok(format!("表の名前を {value} に変えました"));
    };
    let old = d.tables[table]
        .columns
        .get(ci)
        .ok_or("列が見つかりません")?
        .name
        .clone();
    match (field, labels) {
        (Field::Type, _) => {
            if value.is_empty() {
                return Err("型が空です".into());
            }
            d.tables[table].columns[ci].ty = DataType::parse(value);
            Ok(format!("{old} の型を {value} にしました"))
        }
        (Field::Name, Labels::Logical) => {
            d.tables[table].columns[ci].logical = value.to_string();
            Ok(format!("{old} の論理名を変えました"))
        }
        (Field::Name, Labels::Physical) => {
            if !dsl::is_valid_name(value) {
                return Err(format!(
                    "列の名前 '{value}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
                ));
            }
            columns::rename_column(d, table, &old, value)?;
            Ok(format!("列の名前を {old} → {value} に変えました"))
        }
    }
}

impl Designer {
    /// 図の上の欄を、その場で編集できるようにする。つなぐ操作の最中は、何もしない。
    pub(super) fn begin_inline(
        &mut self,
        table: usize,
        column: Option<usize>,
        field: Field,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connect || self.error.is_some() {
            return;
        }
        if self
            .inline
            .as_ref()
            .is_some_and(|i| i.is(table, column, field))
        {
            return;
        }
        self.commit_inline(window, cx);
        if table >= self.design.tables.len() {
            return;
        }
        let text = initial(&self.design, self.labels, table, column, field);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |s, cx| {
            s.set_value(InputContent::from(text), window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_inline(window, cx);
            }
        });
        self.inline = Some(Inline {
            table,
            column,
            field,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    /// その場編集を確定する。変わっていなければ、履歴にも保存にも残さない。
    pub(super) fn commit_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(inline) = self.inline.take() else {
            return;
        };
        let value = inline.input.read(cx).value().trim().to_string();
        let target = (inline.table, inline.column, inline.field);
        let labels = self.labels;
        let mut probe = self.design.clone();
        if apply(&mut probe, labels, target, &value).is_ok() && probe == self.design {
            cx.notify();
            return;
        }
        self.edit(window, cx, |d| apply(d, labels, target, &value));
    }

    /// Esc でやめる(入力は捨てる)。Tab で確定して、次の列(Shift なら前の列)の名前へ移る。
    pub(super) fn inline_key(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.inline.as_ref().map(|i| (i.table, i.column)) else {
            return;
        };
        if key == "escape" {
            self.inline = None;
            cx.notify();
            return;
        }
        self.commit_inline(window, cx);
        let (table, column) = at;
        let count = self.design.tables.get(table).map_or(0, |t| t.columns.len());
        let next = match (column, shift) {
            (None, false) => (count > 0).then_some(0),
            (Some(c), false) => (c + 1 < count).then_some(c + 1),
            (Some(c), true) => c.checked_sub(1),
            (None, true) => None,
        };
        if let Some(next) = next {
            self.begin_inline(table, Some(next), Field::Name, window, cx);
        }
    }

    /// 表の末尾に列を足して、その名前をすぐ編集できるようにする。
    pub(super) fn add_column(&mut self, table: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inline(window, cx);
        let mut added = None;
        self.edit(window, cx, |d| {
            let name = columns::add_column(d, table)?;
            added = Some(d.tables[table].columns.len() - 1);
            Ok(format!("列 {name} を追加しました"))
        });
        if let Some(ci) = added {
            self.begin_inline(table, Some(ci), Field::Name, window, cx);
        }
    }
}
