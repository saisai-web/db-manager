//! 設計の状態の更新。画面の操作(図・テキスト・ボタン)は、ここを通って設計とテキストを同じにする。

use std::time::{Duration, Instant};

use gpui_kit::component::input::{EditorState, InputContent, InputEvent, InputState, Position};
use sqltool_core::design::ddl::{Ddl, DdlOptions, Target, generate};
use sqltool_core::design::diagram::{Labels, geometry};
use sqltool_core::design::edit::{self, Pick};
use sqltool_core::design::{Design, dsl, validate};

use super::{Derived, Designer, SidePanel, diagram};
use crate::model::AppModel;
use crate::ui::prelude::*;

/// 新しい設計の見本。書き方がそのまま分かるようにしてある。
pub(super) const TEMPLATE: &str = "\
@title 新しい設計

customers \"顧客\"
  id    int          pk auto \"顧客ID\"
  name  varchar(100) not null \"氏名\"
  email varchar(255) unique

orders \"注文\"
  id          bigint pk auto
  customer_id int not null -> customers.id on delete cascade \"顧客\"
  placed_at   datetime not null default now() \"注文日時\"
";

/// 元に戻す操作を、連続した入力 1 回ぶんにまとめる間隔。
const COALESCE: Duration = Duration::from_millis(800);
const UNDO_LIMIT: usize = 200;
pub(super) const MIN_ZOOM: f32 = 0.2;
pub(super) const MAX_ZOOM: f32 = 3.0;

impl Designer {
    pub fn new(
        model: Entity<AppModel>,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(false)
                .placeholder("表と列を、1 行 1 列で書きます")
        });
        let schema_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("スキーマ名(省略可)"));
        cx.subscribe_in(&text, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_text_changed(window, cx);
            }
        })
        .detach();
        cx.subscribe_in(
            &schema_input,
            window,
            |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.options.schema = input.read(cx).value().trim().to_string();
                    this.refresh();
                    cx.notify();
                }
            },
        )
        .detach();
        let design = dsl::parse(TEMPLATE).unwrap_or_default();
        let mut me = Self {
            model,
            visible: true,
            derived: derive(
                &design,
                Labels::Physical,
                target,
                &DdlOptions::default(),
                false,
            ),
            design,
            text,
            schema_input,
            last_text: String::new(),
            error: None,
            selected: None,
            connect: false,
            pick: None,
            drag: None,
            resize: None,
            inline: None,
            detail: None,
            relation: None,
            type_menu: None,
            show_text: false,
            export_open: false,
            zoom: 1.0,
            labels: Labels::Physical,
            target,
            options: DdlOptions::default(),
            panel: SidePanel::Sql,
            appendix_ddl: true,
            undo: Vec::new(),
            redo: Vec::new(),
            last_record: None,
            file: None,
            dirty: false,
            message: None,
            draft_seq: 0,
            autosave: !cfg!(test),
        };
        me.write_text(window, cx);
        me.fit(window, 1.0);
        me
    }

    pub(super) fn refresh(&mut self) {
        self.derived = derive(
            &self.design,
            self.labels,
            self.target,
            &self.options,
            self.export_open,
        );
    }

    pub(super) fn note(&mut self, error: bool, text: impl Into<String>) {
        self.message = Some((error, text.into()));
    }

    /// 設計が変わったあとの共通の始末: 描画用の値を作り直し、下書きを保存する。
    pub(super) fn after_change(&mut self, cx: &mut Context<Self>) {
        self.refresh();
        if self
            .selected
            .as_ref()
            .is_some_and(|s| self.design.table(s).is_none())
        {
            self.selected = None;
        }
        self.pick = None;
        self.schedule_draft(cx);
        cx.notify();
    }

    /// 設計をテキストにする。テキストを開いているときは、エディタにも書く
    /// (閉じているときは、開くときに書く。設計を触るたびに、見えないエディタを更新しない)。
    pub(super) fn write_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = dsl::print(&self.design);
        self.last_text = t.clone();
        self.error = None;
        if self.show_text {
            self.text
                .update(cx, |e, cx| e.set_value(InputContent::from(t), window, cx));
        }
    }

    pub(super) fn set_show_text(
        &mut self,
        show: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_text = show;
        if show {
            let t = self.last_text.clone();
            self.error = None;
            self.text
                .update(cx, |e, cx| e.set_value(InputContent::from(t), window, cx));
        }
        cx.notify();
    }

    pub(super) fn on_text_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let value = self.text.read(cx).value().to_string();
        if value == self.last_text {
            self.error = None;
            cx.notify();
            return;
        }
        match dsl::parse(&value) {
            Ok(design) => {
                if self.last_record.is_none_or(|t| t.elapsed() > COALESCE) {
                    self.record(self.last_text.clone());
                }
                self.last_record = Some(Instant::now());
                self.design = design;
                self.last_text = value;
                self.error = None;
                self.dirty = true;
                self.after_change(cx);
            }
            Err(errs) => {
                let more = errs.len().saturating_sub(1);
                self.error = Some(if more == 0 {
                    errs[0].to_string()
                } else {
                    format!("{}(ほか {more} 件)", errs[0])
                });
            }
        }
        cx.notify();
    }

    /// 元に戻す履歴に、変更前のテキストを積む。
    pub(super) fn record(&mut self, before: String) {
        if self.undo.last() != Some(&before) {
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// 設計を変更する。変更に失敗したら(`Err`)、設計は変えず、理由を知らせる。
    pub(super) fn edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Design) -> Result<String, String>,
    ) {
        if self.error.is_some() {
            self.note(true, "テキストに誤りがあります。直してから操作してください");
            cx.notify();
            return;
        }
        let mut next = self.design.clone();
        match change(&mut next) {
            Ok(message) => {
                self.record(self.last_text.clone());
                self.last_record = None;
                self.design = next;
                self.write_text(window, cx);
                self.dirty = true;
                self.note(false, message);
                self.after_change(cx);
            }
            Err(message) => {
                self.note(true, message);
                cx.notify();
            }
        }
    }

    /// 入力欄に打つたびに設計を変える操作。連続した入力は元に戻す履歴 1 回ぶんにまとめ、
    /// 変わらなければ何もしない。失敗したら(`Err`)、設計は変えず、理由を知らせる。
    pub(super) fn edit_live(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Design) -> Result<(), String>,
    ) {
        if self.error.is_some() {
            self.note(true, "テキストに誤りがあります。直してから操作してください");
            cx.notify();
            return;
        }
        let mut next = self.design.clone();
        match change(&mut next) {
            Ok(()) if next == self.design => {}
            Ok(()) => {
                if self.last_record.is_none_or(|t| t.elapsed() > COALESCE) {
                    self.record(self.last_text.clone());
                }
                self.last_record = Some(Instant::now());
                self.design = next;
                self.write_text(window, cx);
                self.dirty = true;
                self.message = None;
                self.after_change(cx);
            }
            Err(message) => {
                self.note(true, message);
                cx.notify();
            }
        }
    }

    /// テキストを丸ごと差し替える(新規・開く・元に戻す)。誤りがあれば、何もせず誤りを返す。
    pub(super) fn replace_text(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let design = dsl::parse(&text).map_err(|e| e[0].to_string())?;
        self.design = design;
        self.write_text(window, cx);
        self.selected = None;
        self.inline = None;
        self.detail = None;
        self.relation = None;
        self.type_menu = None;
        self.after_change(cx);
        Ok(())
    }

    pub(super) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = self.undo.pop() else { return };
        let now = self.last_text.clone();
        if self.replace_text(prev.clone(), window, cx).is_ok() {
            self.redo.push(now);
            self.dirty = true;
            self.last_record = None;
        } else {
            self.undo.push(prev);
        }
    }

    pub(super) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.redo.pop() else { return };
        let now = self.last_text.clone();
        if self.replace_text(next.clone(), window, cx).is_ok() {
            self.undo.push(now);
            self.dirty = true;
            self.last_record = None;
        } else {
            self.redo.push(next);
        }
    }

    // ---- 操作 ----

    pub(super) fn add_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.design.unused_table_name("table");
        self.edit(window, cx, |d| {
            Ok(format!("表 {} を追加しました", edit::add_table(d, "table")))
        });
        if self.design.table(&name).is_some() {
            self.select(&name, window, cx);
        }
    }

    pub(super) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.edit(window, cx, |d| {
            d.remove_table(&name);
            Ok(format!(
                "表 {name} を削除しました(この表を参照する関係も消えました)"
            ))
        });
    }

    pub(super) fn auto_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(window, cx, |d| {
            d.tables.iter_mut().for_each(|t| t.pos = None);
            Ok("自動で配置しました".into())
        });
    }

    pub(super) fn select(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(name.to_string());
        // テキストを開いているときは、その表の行へカーソルを移す
        let line = self.show_text.then(|| {
            self.last_text.lines().position(|l| {
                !l.starts_with([' ', '\t']) && l.split_whitespace().next() == Some(name)
            })
        });
        if let Some(line) = line.flatten() {
            self.text.update(cx, |e, cx| {
                e.set_cursor_position(Position::new(line as u32, 0), window, cx)
            });
        }
        cx.notify();
    }

    pub(super) fn click(
        &mut self,
        table: &str,
        column: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connect {
            self.select(table, window, cx);
            return;
        }
        let picked = Pick {
            table: table.into(),
            column: column.map(String::from),
        };
        match self.pick.take() {
            None => {
                self.pick = Some(picked);
                self.note(
                    false,
                    "つなぐ相手(参照される側の表または列)をクリックしてください",
                );
                cx.notify();
            }
            Some(first) if first == picked => {
                self.note(false, "選択を取り消しました");
                cx.notify();
            }
            Some(first) => {
                self.edit(window, cx, |d| {
                    edit::connect(d, &first, &picked)
                        .map(|(c, cc, p, pc)| format!("関係を追加しました: {c}.{cc} → {p}.{pc}"))
                });
            }
        }
    }

    pub(super) fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        cx.notify();
    }

    /// 図全体が画面に収まる倍率にする。`max` は、小さな図を拡大してよい上限(1.0 なら、拡大しない)。
    pub(super) fn fit(&mut self, window: &Window, max: f32) {
        let view = window.viewport_size();
        // ツールバーと下の帯、図の余白を除いた、図の領域
        let (w, h) = (f32::from(view.width) - 16.0, f32::from(view.height) - 90.0);
        let (gw, gh) = (
            self.derived.geo.size.0 + 2.0 * diagram::MARGIN,
            self.derived.geo.size.1 + 2.0 * diagram::MARGIN,
        );
        self.zoom = (w / gw).min(h / gh).clamp(MIN_ZOOM, max);
    }

    pub(super) fn set_labels(&mut self, labels: Labels, cx: &mut Context<Self>) {
        self.labels = labels;
        self.refresh();
        cx.notify();
    }

    pub(super) fn set_target(&mut self, target: Target, cx: &mut Context<Self>) {
        self.target = target;
        self.refresh();
        cx.notify();
    }
}

/// 画面に出す CREATE 文の最大行数(表が多い設計でも、描画が重くならないように)。
const PREVIEW_LINES: usize = 500;

fn preview_of(sql: &str) -> SharedString {
    let total = sql.lines().count();
    if total <= PREVIEW_LINES {
        return sql.to_string().into();
    }
    let head: Vec<&str> = sql.lines().take(PREVIEW_LINES).collect();
    format!(
        "{}\n\n-- …以降 {} 行は省略しています(コピー・保存には全文が入ります)\n",
        head.join("\n"),
        total - PREVIEW_LINES
    )
    .into()
}

fn derive(
    design: &Design,
    labels: Labels,
    target: Target,
    options: &DdlOptions,
    with_sql: bool,
) -> Derived {
    let ddl = if with_sql {
        generate(design, target, options)
    } else {
        Ddl::default()
    };
    Derived {
        geo: geometry(design, labels),
        sql: ddl.sql.clone().into(),
        preview: preview_of(&ddl.sql),
        issues: validate(design),
        ddl,
    }
}
