//! 型の選択。型の欄をクリックすると、よく使う型の一覧が開く(表の詳細エディタの「▾」でも同じ)。
//! 一覧にない型・長さは「直接入力…」で書く。

use sqltool_core::design::DataType;
use sqltool_core::design::diagram::{HEADER_H, row_height};

use super::Designer;
use super::diagram::MARGIN;
use super::inline::Field;
use crate::ui::prelude::*;

/// 開いている型の一覧。
pub(in crate::ui) struct TypeMenu {
    pub table: usize,
    pub col: usize,
    /// 表の詳細エディタから開いたか(図の上ではなく、中央に出す)
    pub in_dialog: bool,
}

/// 一覧に出す型(区分ごと)。書き方は、設計のテキスト記法と同じ。
const GROUPS: [(&str, &[&str]); 5] = [
    ("整数", &["smallint", "int", "bigint"]),
    (
        "小数",
        &["decimal(10,2)", "decimal(18,4)", "float", "double"],
    ),
    (
        "文字",
        &[
            "char(1)",
            "char(10)",
            "varchar(50)",
            "varchar(100)",
            "varchar(255)",
            "varchar(1000)",
            "text",
        ],
    ),
    ("日付・時刻", &["date", "time", "datetime", "timestamptz"]),
    ("その他", &["bool", "uuid", "json", "blob"]),
];

fn flat() -> impl Iterator<Item = &'static str> {
    GROUPS.iter().flat_map(|(_, items)| items.iter().copied())
}

/// 一覧の `index` 番目の型。
pub(super) fn choice(index: usize) -> Option<&'static str> {
    flat().nth(index)
}

/// 一覧にある型の位置(テスト用)。
#[cfg(test)]
pub(super) fn position_of(ty: &str) -> Option<usize> {
    flat().position(|t| t == ty)
}

impl Designer {
    pub(super) fn open_type_menu(
        &mut self,
        table: usize,
        col: usize,
        in_dialog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connect || self.error.is_some() {
            return;
        }
        self.commit_inline(window, cx);
        self.type_menu = Some(TypeMenu {
            table,
            col,
            in_dialog,
        });
        cx.notify();
    }

    pub(super) fn close_type_menu(&mut self, cx: &mut Context<Self>) {
        if self.type_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn choose_type(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(menu), Some(label)) = (self.type_menu.take(), choice(index)) else {
            return;
        };
        let ty = DataType::parse(label);
        self.edit(window, cx, |d| {
            let c = d
                .tables
                .get_mut(menu.table)
                .and_then(|t| t.columns.get_mut(menu.col))
                .ok_or("列が見つかりません")?;
            c.ty = ty;
            Ok(format!("{} の型を {label} にしました", c.name))
        });
        if menu.in_dialog {
            self.rebuild_detail(window, cx);
        }
    }

    /// 一覧にない型を、その場で書く。
    pub(super) fn type_menu_other(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.type_menu.take() {
            self.begin_inline(menu.table, Some(menu.col), Field::Type, window, cx);
        }
    }
}

/// 型の一覧の中身。
pub(super) fn type_panel(d: &Designer, cx: &mut Context<Designer>) -> Option<AnyElement> {
    let menu = d.type_menu.as_ref()?;
    let col = d.design.tables.get(menu.table)?.columns.get(menu.col)?;
    let current = col.ty.to_string();
    let theme = cx.theme().clone();
    let mut index = 0;
    let mut groups = Vec::new();
    for (title, items) in GROUPS {
        let chips: Vec<_> = items
            .iter()
            .map(|&label| {
                let i = index;
                index += 1;
                let on = DataType::parse(label).to_string() == current;
                div()
                    .id(("ty", i))
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .border_1()
                    .border_color(if on { theme.primary } else { theme.border })
                    .when(on, |c| c.bg(theme.primary.opacity(0.2)))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|h| h.bg(theme.accent))
                    .on_click(cx.listener(move |this, _, w, cx| this.choose_type(i, w, cx)))
                    .child(label)
                    .test_support()
            })
            .collect();
        groups.push(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(title),
                )
                .child(div().flex().flex_row().flex_wrap().gap_1().children(chips)),
        );
    }
    Some(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} の型(いまは {current})", col.name)),
            )
            .children(groups)
            .when(!menu.in_dialog, |p| {
                p.child(
                    Button::new("ty-other")
                        .label("直接入力…")
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, w, cx| this.type_menu_other(w, cx))),
                )
            })
            .into_any_element(),
    )
}

/// 図の上に出す一覧(その列の行のすぐ下)。図の中身の座標で置くので、スクロールにもついてくる。
pub(super) fn type_popup(d: &Designer, cx: &mut Context<Designer>) -> Option<AnyElement> {
    let menu = d.type_menu.as_ref().filter(|m| !m.in_dialog)?;
    let panel = type_panel(d, cx)?;
    let theme = cx.theme().clone();
    let r = *d.derived.geo.rects.get(menu.table)?;
    let k = d.design.tables.get(menu.table)?.factor();
    let left = (r.x + MARGIN) * d.zoom + r.w * d.zoom * 0.35;
    let top =
        (r.y + MARGIN) * d.zoom + (HEADER_H + row_height(k) * (menu.col as f32 + 1.0)) * d.zoom;
    Some(
        div()
            .id("type-popup")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(330.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .child(panel)
            .into_any_element(),
    )
}
