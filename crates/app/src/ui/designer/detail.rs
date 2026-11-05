//! 表の詳細エディタ(ダブルクリックか「詳細」で開く)。表の名前と、列ごとの名前・型・制約などをすべて編集できる。
//! 入力欄に打つたびに設計へ反映する(連続した入力は、元に戻す履歴 1 回ぶん)。

use gpui_kit::component::input::{InputContent, InputEvent, InputState};
use sqltool_core::design::{DataType, columns, dsl};

use super::Designer;
use crate::ui::prelude::*;

/// 列 1 つぶんの入力欄。
pub(in crate::ui) struct ColumnInputs {
    pub name: Entity<InputState>,
    pub logical: Entity<InputState>,
    pub ty: Entity<InputState>,
    pub default: Entity<InputState>,
    pub comment: Entity<InputState>,
}

pub(in crate::ui) struct Detail {
    /// `design.tables` の添字
    pub table: usize,
    pub name: Entity<InputState>,
    pub logical: Entity<InputState>,
    pub comment: Entity<InputState>,
    pub cols: Vec<ColumnInputs>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum Flag {
    Primary,
    NotNull,
    Unique,
}

/// 入力欄をつくり、打つたびに設計へ反映するよう結びつける。
fn field(
    text: &str,
    placeholder: &'static str,
    subs: &mut Vec<Subscription>,
    window: &mut Window,
    cx: &mut Context<Designer>,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    input.update(cx, |s, cx| {
        s.set_value(InputContent::from(text.to_string()), window, cx)
    });
    subs.push(
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.detail_changed(window, cx);
            }
        }),
    );
    input
}

impl Designer {
    pub(super) fn open_detail(
        &mut self,
        table: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.error.is_some() || table >= self.design.tables.len() {
            return;
        }
        self.commit_inline(window, cx);
        self.drag = None;
        self.detail = Some(self.build_detail(table, window, cx));
        cx.notify();
    }

    pub(super) fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        cx.notify();
    }

    fn build_detail(&self, table: usize, window: &mut Window, cx: &mut Context<Self>) -> Detail {
        let t = &self.design.tables[table];
        let mut subs = Vec::new();
        let name = field(&t.name, "物理名(表名)", &mut subs, window, cx);
        let logical = field(&t.logical, "論理名(日本語名など)", &mut subs, window, cx);
        let comment = field(&t.comment, "備考・用途", &mut subs, window, cx);
        let cols = t
            .columns
            .iter()
            .map(|c| ColumnInputs {
                name: field(&c.name, "列名", &mut subs, window, cx),
                logical: field(&c.logical, "論理名", &mut subs, window, cx),
                ty: field(
                    &c.ty.to_string(),
                    "型(int, varchar(100) …)",
                    &mut subs,
                    window,
                    cx,
                ),
                default: field(
                    c.default.as_deref().unwrap_or(""),
                    "既定値",
                    &mut subs,
                    window,
                    cx,
                ),
                comment: field(&c.comment, "備考", &mut subs, window, cx),
            })
            .collect();
        Detail {
            table,
            name,
            logical,
            comment,
            cols,
            _subs: subs,
        }
    }

    /// 列の追加・削除・並べ替えのあと、入力欄を設計に合わせて作り直す。
    pub(super) fn rebuild_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        self.detail =
            (table < self.design.tables.len()).then(|| self.build_detail(table, window, cx));
        cx.notify();
    }

    /// 入力欄のどれかが変わった。全部の欄を読んで、設計に反映する。
    pub(super) fn detail_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let text = |e: &Entity<InputState>| e.read(cx).value().to_string();
        let table = detail.table;
        let (name, logical, comment) = (
            text(&detail.name).trim().to_string(),
            text(&detail.logical),
            text(&detail.comment),
        );
        let cols: Vec<[String; 5]> = detail
            .cols
            .iter()
            .map(|c| {
                [
                    text(&c.name).trim().to_string(),
                    text(&c.logical),
                    text(&c.ty).trim().to_string(),
                    text(&c.default).trim().to_string(),
                    text(&c.comment),
                ]
            })
            .collect();
        let old_name = self.design.tables.get(table).map(|t| t.name.clone());
        self.edit_live(window, cx, move |d| {
            let old = d.tables.get(table).ok_or("表が見つかりません")?.name.clone();
            if name != old {
                if !dsl::is_valid_name(&name) {
                    return Err(format!(
                        "表の名前 '{name}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
                    ));
                }
                if d.table(&name).is_some() {
                    return Err(format!("表 {name} は、すでにあります"));
                }
                d.rename_table(&old, &name);
            }
            let t = &mut d.tables[table];
            t.logical = logical;
            t.comment = comment;
            for (j, [cname, clogical, ty, default, ccomment]) in cols.into_iter().enumerate() {
                let current = d.tables[table]
                    .columns
                    .get(j)
                    .ok_or("列が見つかりません")?
                    .name
                    .clone();
                if cname != current {
                    if !dsl::is_valid_name(&cname) {
                        return Err(format!(
                            "列の名前 '{cname}' は使えません(空白・引用符・括弧・ドットなどを含まない)"
                        ));
                    }
                    columns::rename_column(d, table, &current, &cname)?;
                }
                let c = &mut d.tables[table].columns[j];
                c.logical = clogical;
                c.comment = ccomment;
                if !ty.is_empty() && c.ty.to_string() != ty {
                    c.ty = DataType::parse(&ty);
                }
                c.default = (!default.is_empty()).then_some(default);
            }
            Ok(())
        });
        if let (Some(old), Some(t)) = (old_name, self.design.tables.get(table))
            && self.selected.as_deref() == Some(old.as_str())
        {
            self.selected = Some(t.name.clone());
        }
    }

    /// 主キー・NOT NULL・一意の切り替え。
    pub(super) fn toggle_flag(
        &mut self,
        col: usize,
        flag: Flag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        self.edit(window, cx, |d| {
            let c = d
                .tables
                .get_mut(table)
                .and_then(|t| t.columns.get_mut(col))
                .ok_or("列が見つかりません")?;
            match flag {
                Flag::Primary => {
                    c.primary = !c.primary;
                    if c.primary {
                        c.nullable = false;
                    }
                }
                Flag::NotNull if c.primary => {
                    return Err("主キーの列は、NULL にできません".into());
                }
                Flag::NotNull => c.nullable = !c.nullable,
                Flag::Unique => c.unique = !c.unique,
            }
            Ok(format!("列 {} の指定を変えました", c.name))
        });
    }

    pub(super) fn detail_add_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        self.edit(window, cx, |d| {
            let name = columns::add_column(d, table)?;
            Ok(format!("列 {name} を追加しました"))
        });
        self.rebuild_detail(window, cx);
    }

    pub(super) fn detail_remove_column(
        &mut self,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        self.edit(window, cx, |d| {
            let name = d
                .tables
                .get(table)
                .and_then(|t| t.columns.get(col))
                .map(|c| c.name.clone())
                .ok_or("列が見つかりません")?;
            columns::remove_column(d, table, &name)?;
            Ok(format!(
                "列 {name} を削除しました(この列を使う関係・索引も消えました)"
            ))
        });
        self.rebuild_detail(window, cx);
    }

    pub(super) fn detail_move_column(
        &mut self,
        col: usize,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        self.edit(window, cx, |d| {
            columns::move_column(d, table, col, delta)?;
            Ok("列の順番を変えました".into())
        });
        self.rebuild_detail(window, cx);
    }

    /// 表の大きさを `delta` だけ変える(`None` なら等倍に戻す)。
    pub(super) fn detail_scale(
        &mut self,
        delta: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        let Some(t) = self.design.tables.get(table) else {
            return;
        };
        let next = delta.map_or(1.0, |d| t.factor() + d);
        self.record(self.last_text.clone());
        self.last_record = None;
        // 位置が自動のままだと、大きさを変えたときに他の表まで動くので、いまの位置に固定する
        for (t, r) in self.design.tables.iter_mut().zip(&self.derived.geo.rects) {
            t.pos.get_or_insert((r.x.round(), r.y.round()));
        }
        self.set_scale(table, next);
        self.write_text(window, cx);
        self.dirty = true;
        self.after_change(cx);
    }

    pub(super) fn detail_delete_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.detail.as_ref().map(|d| d.table) else {
            return;
        };
        let Some(name) = self.design.tables.get(table).map(|t| t.name.clone()) else {
            return;
        };
        self.detail = None;
        self.edit(window, cx, |d| {
            d.remove_table(&name);
            Ok(format!(
                "表 {name} を削除しました(この表を参照する関係も消えました)"
            ))
        });
    }
}
