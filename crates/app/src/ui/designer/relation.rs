//! 関係線の詳細。線をダブルクリックすると、名前・削除時/更新時の動き・親が必須か・1 対 1 かを設定できる。

use gpui_kit::component::input::{InputContent, InputEvent, InputState};
use sqltool_core::design::{RefAction, relations};

use super::Designer;
use super::diagram::MARGIN;
use crate::ui::prelude::*;

/// 開いている関係。`table` は子の表、`fk` はその `foreign_keys` の添字。
pub(in crate::ui) struct Relation {
    pub table: usize,
    pub fk: usize,
    pub name: Entity<InputState>,
    _sub: Subscription,
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum Toggle {
    Required,
    OneToOne,
}

/// 線をつかみやすくする、見えない帯の太さ(画面上の px)
const GRAB: f32 = 12.0;

impl Designer {
    pub(super) fn open_relation(
        &mut self,
        table: usize,
        fk: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(f) = self
            .design
            .tables
            .get(table)
            .and_then(|t| t.foreign_keys.get(fk))
        else {
            return;
        };
        let name = f.name.clone();
        self.commit_inline(window, cx);
        self.drag = None;
        self.type_menu = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("制約名(空なら自動)"));
        input.update(cx, |s, cx| {
            s.set_value(InputContent::from(name), window, cx)
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.relation_name_changed(window, cx);
            }
        });
        self.relation = Some(Relation {
            table,
            fk,
            name: input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(super) fn close_relation(&mut self, cx: &mut Context<Self>) {
        self.relation = None;
        cx.notify();
    }

    fn relation_at(&self) -> Option<(usize, usize)> {
        self.relation.as_ref().map(|r| (r.table, r.fk))
    }

    pub(super) fn relation_name_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.relation else { return };
        let (table, fk) = (r.table, r.fk);
        let name = r.name.read(cx).value().to_string();
        self.edit_live(window, cx, move |d| {
            relations::set_name(d, table, fk, &name)
        });
    }

    pub(super) fn relation_action(
        &mut self,
        on_delete: bool,
        action: RefAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table, fk)) = self.relation_at() else {
            return;
        };
        self.edit(window, cx, |d| {
            relations::set_action(d, table, fk, on_delete, action)?;
            let when = if on_delete { "削除時" } else { "更新時" };
            Ok(format!("{when}の動きを {} にしました", action.sql()))
        });
    }

    pub(super) fn relation_toggle(
        &mut self,
        toggle: Toggle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((table, fk)) = self.relation_at() else {
            return;
        };
        self.edit(window, cx, |d| {
            let t = d.tables.get(table).ok_or("表が見つかりません")?;
            let f = t.foreign_keys.get(fk).ok_or("関係が見つかりません")?;
            match toggle {
                Toggle::Required => {
                    let on = !t.fk_required(f);
                    relations::set_required(d, table, fk, on)?;
                    Ok(if on {
                        "親が必須(子の列は NOT NULL)にしました"
                    } else {
                        "親は任意(子の列は NULL 可)にしました"
                    }
                    .into())
                }
                Toggle::OneToOne => {
                    let on = !t.fk_is_one_to_one(f);
                    relations::set_one_to_one(d, table, fk, on)?;
                    Ok(if on {
                        "1 対 1(子の列は一意)にしました"
                    } else {
                        "1 対多にしました"
                    }
                    .into())
                }
            }
        });
    }

    pub(super) fn relation_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((table, fk)) = self.relation_at() else {
            return;
        };
        self.relation = None;
        self.edit(window, cx, |d| {
            relations::remove(d, table, fk)?;
            Ok("関係を消しました(列は残っています)".into())
        });
    }
}

/// 関係線の上に重ねる、見えない帯。ダブルクリックで詳細を開く。表の箱より下に置く。
pub(super) fn edge_hits(d: &Designer, cx: &mut Context<Designer>) -> Vec<AnyElement> {
    let z = d.zoom;
    let accent = cx.theme().primary.opacity(0.25);
    let mut hits = Vec::new();
    for (ei, e) in d.derived.geo.edges.iter().enumerate() {
        let (table, fk) = (e.child, e.fk);
        for (si, w) in e.points.windows(2).enumerate() {
            let at = |p: (f32, f32)| ((p.0 + MARGIN) * z, (p.1 + MARGIN) * z);
            let ((x1, y1), (x2, y2)) = (at(w[0]), at(w[1]));
            let (left, top) = (x1.min(x2) - GRAB / 2.0, y1.min(y2) - GRAB / 2.0);
            let (width, height) = ((x1 - x2).abs() + GRAB, (y1 - y2).abs() + GRAB);
            hits.push(
                div()
                    .id(("rel", ei * 8 + si))
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .h(px(height))
                    .cursor_pointer()
                    .rounded(px(3.))
                    .hover(move |h| h.bg(accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            if ev.click_count >= 2 {
                                this.open_relation(table, fk, w, cx);
                            }
                        }),
                    )
                    .test_support()
                    .into_any_element(),
            );
        }
    }
    hits
}
