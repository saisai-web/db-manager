//! ファイルの読み書き(設計・CREATE 文・設計書)、下書きの自動保存、SQL エディタへの受け渡し。

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::InputContent;
use sqltool_core::design::ddl::{DdlOptions, Target, generate};
use sqltool_core::design::diagram::Labels;
use sqltool_core::design::docs::{self, DocOptions};
use sqltool_core::design::{Design, dsl, svg};

use super::Designer;
use super::state::TEMPLATE;
use crate::ui::prelude::*;

#[derive(Clone, Copy)]
pub(super) enum Export {
    Sql,
    Xlsx,
    Html,
    Svg,
}

impl Export {
    fn extension(self) -> &'static str {
        match self {
            Self::Sql => "sql",
            Self::Xlsx => "xlsx",
            Self::Html => "html",
            Self::Svg => "svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sql => "CREATE 文",
            Self::Xlsx => "Excel の設計書",
            Self::Html => "HTML の設計書",
            Self::Svg => "ER 図(SVG)",
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// 書き出す中身。画面のスレッドを止めないよう、バックグラウンドで作る。
pub(super) struct Job {
    pub kind: Export,
    pub design: Design,
    pub target: Target,
    pub ddl: DdlOptions,
    pub docs: DocOptions,
}

impl Job {
    pub(super) fn bytes(&self) -> Result<Vec<u8>, String> {
        let docs = DocOptions {
            date: docs::today(),
            ..self.docs.clone()
        };
        let docs = &docs;
        Ok(match self.kind {
            Export::Sql => generate(&self.design, self.target, &self.ddl)
                .sql
                .into_bytes(),
            Export::Xlsx => docs::xlsx(&self.design, docs)?,
            Export::Html => docs::html(&self.design, docs).into_bytes(),
            Export::Svg => svg::svg_file(&self.design, docs.labels).into_bytes(),
        })
    }
}

impl Designer {
    fn base_name(&self) -> String {
        self.file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "design".into())
    }

    fn start_dir(&self) -> PathBuf {
        self.file
            .as_ref()
            .and_then(|f| f.parent())
            .map_or_else(home, PathBuf::from)
    }

    pub(super) fn export(&mut self, kind: Export, window: &mut Window, cx: &mut Context<Self>) {
        let job = Job {
            kind,
            design: self.design.clone(),
            target: self.target,
            ddl: self.options.clone(),
            docs: DocOptions {
                date: String::new(), // 書き出すときに、バックグラウンドで入れる
                ddl: self.appendix_ddl.then_some(self.target),
                labels: self.labels,
            },
        };
        let name = format!("{}.{}", self.base_name(), kind.extension());
        let rx = cx.prompt_for_new_path(&self.start_dir(), Some(&name));
        let exec = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let result = exec
                .spawn(async move {
                    let bytes = job.bytes()?;
                    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok::<_, String>(path)
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(p) => this.note(
                        false,
                        format!("{}を保存しました: {}", kind.label(), p.display()),
                    ),
                    Err(e) => this.note(true, format!("保存できません: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_sql(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.derived.sql.to_string()));
        self.note(false, "CREATE 文をコピーしました");
        cx.notify();
    }

    /// CREATE 文を、開いている接続の SQL エディタに入れる(実行はしない)。
    pub(super) fn send_to_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sql = self.derived.sql.to_string();
        let editor = self.model.read(cx).active_tab().map(|t| t.editor().clone());
        let Some(editor) = editor else {
            self.note(
                true,
                "SQL を入れるエディタがありません。先に接続を開いてください",
            );
            cx.notify();
            return;
        };
        editor.update(cx, |e, cx| e.set_value(InputContent::from(sql), window, cx));
        self.visible = false;
        self.close_export(cx);
        cx.notify();
        self.model.update(cx, |m, cx| {
            m.notice = Some(crate::model::StatusLine::ok(
                "CREATE 文を SQL エディタに入れました。内容を確認して実行してください",
            ));
            cx.notify();
        });
    }

    // ---- 設計のファイル ----

    pub(super) fn new_design(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.record(self.last_text.clone());
        if self.replace_text(TEMPLATE.to_string(), window, cx).is_ok() {
            self.file = None;
            self.dirty = false;
            self.fit(window, 1.0);
            self.note(
                false,
                "新しい設計を始めました(⌘Z / 元に戻すで、前の内容に戻れます)",
            );
        }
        cx.notify();
    }

    pub(super) fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("開く設計(.erd)を選択".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.open_path(path, window, cx));
        })
        .detach();
    }

    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.note(true, format!("{}: {e}", path.display()));
                cx.notify();
                return;
            }
        };
        self.record(self.last_text.clone());
        match self.replace_text(text, window, cx) {
            Ok(()) => {
                self.note(false, format!("開きました: {}", path.display()));
                self.file = Some(path);
                self.dirty = false;
                self.fit(window, 1.0);
            }
            Err(e) => self.note(true, format!("{}: {e}", path.display())),
        }
        cx.notify();
    }

    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.file.clone() {
            Some(path) => self.save_to(path, cx),
            None => self.save_as(window, cx),
        }
    }

    pub(super) fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_new_path(
            &self.start_dir(),
            Some(&format!("{}.erd", self.base_name())),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let _ = this.update_in(cx, |this, _, cx| this.save_to(path, cx));
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.error.is_some() {
            self.note(true, "テキストに誤りがあります。直してから保存してください");
            cx.notify();
            return;
        }
        let text = dsl::print(&self.design);
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.note(false, format!("保存しました: {}", path.display()));
                self.file = Some(path);
                self.dirty = false;
            }
            Err(e) => self.note(true, format!("保存できません: {}: {e}", path.display())),
        }
        cx.notify();
    }

    // ---- 下書き ----

    /// 前回の続きから始める(保存していない設計が、下書きとして残っている)。
    pub fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // テストでは、利用者の下書きを読まない(結果が、その人の使い方で変わってしまう)
        if !self.autosave {
            return;
        }
        let Some(text) =
            sqltool_core::design_draft_path().and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return;
        };
        if text.trim().is_empty() || self.replace_text(text, window, cx).is_err() {
            return;
        }
        self.dirty = true;
        self.note(false, "前回の続きを開きました(保存していない下書き)");
    }

    pub(super) fn schedule_draft(&mut self, cx: &mut Context<Self>) {
        if !self.autosave {
            return;
        }
        self.draft_seq += 1;
        let seq = self.draft_seq;
        let exec = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            exec.timer(Duration::from_millis(1500)).await;
            let text = this
                .update(cx, |this, _| {
                    (this.draft_seq == seq).then(|| this.last_text.clone())
                })
                .ok()
                .flatten();
            let (Some(text), Some(path)) = (text, sqltool_core::design_draft_path()) else {
                return;
            };
            exec.spawn(async move {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, text);
            })
            .await;
        })
        .detach();
    }

    /// 実際の DB から読んだ設計に差し替える(元に戻せる)。
    pub fn import(
        &mut self,
        design: Design,
        from: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = from.to_string();
        self.visible = true;
        self.edit(window, cx, move |d| {
            let n = design.tables.len();
            *d = design;
            Ok(format!("{from} から {n} 表を取り込みました"))
        });
        self.labels = Labels::Physical;
        self.refresh();
        self.fit(window, 1.0);
        self.file = None;
    }
}
