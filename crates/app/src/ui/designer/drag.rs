//! 表の移動(見出しをドラッグ)と、大きさの変更(右下の角をドラッグ)。

use sqltool_core::design::diagram::geometry;

use super::{Designer, Drag, Resize};
use crate::ui::prelude::*;

/// 大きさの刻み(細かくして、ドラッグになめらかについてくるようにする)。
const SCALE_STEP: f32 = 0.01;

impl Designer {
    /// 自動配置の表が動かないよう、いまの位置を、すべての表に固定する。
    fn pin_positions(&mut self) {
        for (t, r) in self.design.tables.iter_mut().zip(&self.derived.geo.rects) {
            t.pos.get_or_insert((r.x.round(), r.y.round()));
        }
    }

    pub(super) fn begin_drag(
        &mut self,
        table: usize,
        mouse: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.error.is_some() || table >= self.derived.geo.rects.len() {
            return;
        }
        let r = self.derived.geo.rects[table];
        self.drag = Some(Drag {
            table,
            start_mouse: (f32::from(mouse.x), f32::from(mouse.y)),
            start_pos: (r.x, r.y),
            before: self.last_text.clone(),
            moved: false,
        });
        let name = self.design.tables[table].name.clone();
        if !self.connect {
            self.selected = Some(name);
        }
        cx.notify();
    }

    pub(super) fn drag_to(&mut self, mouse: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        let dx = (f32::from(mouse.x) - drag.start_mouse.0) / self.zoom;
        let dy = (f32::from(mouse.y) - drag.start_mouse.1) / self.zoom;
        if !drag.moved && dx.abs() + dy.abs() < 4.0 {
            return;
        }
        let first = !drag.moved;
        drag.moved = true;
        let (table, start) = (drag.table, drag.start_pos);
        if first {
            self.pin_positions();
        }
        // 格子には揃えず、ポインタにそのままついてくる(保存は整数の座標)
        if let Some(t) = self.design.tables.get_mut(table) {
            t.pos = Some((
                (start.0 + dx).round().max(0.0),
                (start.1 + dy).round().max(0.0),
            ));
        }
        self.derived.geo = geometry(&self.design, self.labels);
        cx.notify();
    }

    pub(super) fn end_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take()
            && drag.moved
        {
            self.record(drag.before);
            self.write_text(window, cx);
            self.dirty = true;
            self.after_change(cx);
        }
        if let Some(resize) = self.resize.take()
            && resize.moved
        {
            self.record(resize.before);
            self.write_text(window, cx);
            self.dirty = true;
            self.after_change(cx);
        }
        cx.notify();
    }

    // ---- 表の大きさ ----

    pub(super) fn begin_resize(
        &mut self,
        table: usize,
        mouse: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.error.is_some() || table >= self.derived.geo.rects.len() {
            return;
        }
        self.resize = Some(Resize {
            table,
            start_mouse_x: f32::from(mouse.x),
            start_scale: self.design.tables[table].factor(),
            start_width: self.derived.geo.rects[table].w * self.zoom,
            before: self.last_text.clone(),
            moved: false,
        });
        cx.notify();
    }

    pub(super) fn resize_to(&mut self, mouse: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(r) = &mut self.resize else { return };
        let dx = f32::from(mouse.x) - r.start_mouse_x;
        if !r.moved && dx.abs() < 3.0 {
            return;
        }
        let first = !r.moved;
        r.moved = true;
        let (table, scale) = (
            r.table,
            r.start_scale * (r.start_width + dx) / r.start_width.max(1.0),
        );
        if first {
            self.pin_positions();
        }
        self.set_scale(table, scale);
        cx.notify();
    }

    /// 表の拡大率を決めて、図の形を作り直す(履歴・保存は呼び出し側)。
    pub(super) fn set_scale(&mut self, table: usize, scale: f32) {
        let snapped = (scale / SCALE_STEP).round() * SCALE_STEP;
        if let Some(t) = self.design.tables.get_mut(table) {
            let k = snapped.clamp(
                sqltool_core::design::MIN_SCALE,
                sqltool_core::design::MAX_SCALE,
            );
            t.scale = ((k - 1.0).abs() > 0.001).then_some(k);
        }
        self.derived.geo = geometry(&self.design, self.labels);
    }
}
