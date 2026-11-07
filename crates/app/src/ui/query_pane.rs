//! Query入力。結果の上に並ぶ、SQL を書く窓。上のタブで複数の SQL を切り替える(「＋」で足す)。
//! Tree-sitter による色分けと行番号つき。Cmd/Ctrl + Enter で、カーソル位置の文(選択中なら選択範囲)を実行する。

use gpui_kit::assets::IconName;
use gpui_kit::component::input::Editor;

use super::prelude::*;
use super::theme;
use super::workspace::Workspace;

/// 描画に必要な値だけを写したもの(モデルの借用を、リスナーの登録より前に手放すため)。
struct Snapshot {
    tab: sqltool_core::TabId,
    titles: Vec<SharedString>,
    current: usize,
    editor: Entity<gpui_kit::component::input::EditorState>,
}

fn snapshot(ws: &Workspace, cx: &App) -> Option<Snapshot> {
    let model = ws.model.read(cx);
    if !model.query_open {
        return None;
    }
    let t = model.active_tab()?;
    Some(Snapshot {
        tab: t.id,
        titles: t.queries.iter().map(|q| q.title.clone()).collect(),
        current: t.current_query.min(t.queries.len() - 1),
        editor: t.editor().clone(),
    })
}

pub fn query_pane(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<impl IntoElement + use<>> {
    let s = snapshot(ws, cx)?;
    let theme_colors = cx.theme().clone();
    let tab = s.tab;
    let many = s.titles.len() > 1;
    let chips: Vec<_> = s
        .titles
        .iter()
        .enumerate()
        .map(|(ix, title)| {
            let b = Button::new(("qtab", ix)).small().label(title.clone());
            let b = if ix == s.current {
                b.primary()
            } else {
                b.ghost()
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(b.on_click(cx.listener(move |this, _, window, cx| {
                    this.model
                        .update(cx, |m, cx| m.select_query(tab, ix, window, cx));
                })))
                .when(many, |d| {
                    d.child(
                        Button::new(("qclose", ix))
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("このタブを閉じる")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model
                                    .update(cx, |m, cx| m.close_query_doc(tab, ix, cx));
                            })),
                    )
                })
        })
        .collect();
    Some(
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(32.))
                    .px_2()
                    .border_b_1()
                    .border_color(theme_colors.border)
                    .children(chips)
                    .child(
                        Button::new("qadd")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .tooltip("新しいタブ")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.model
                                    .update(cx, |m, cx| m.add_query(tab, None, None, window, cx));
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("query-close")
                            .icon(IconName::Close)
                            .small()
                            .ghost()
                            .tooltip("Query入力を閉じる")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |m, cx| m.close_query(cx));
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(theme::mono_font())
                    .text_size(px(13.))
                    .child(Editor::new(&s.editor).h_full().appearance(false)),
            ),
    )
}
