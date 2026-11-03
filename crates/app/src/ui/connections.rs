//! 接続先の一覧ダイアログ。接続、編集、削除、新規作成。

use super::prelude::*;
use gpui_kit::component::WindowExt as _;
use sqltool_core::{ConnectionSummary, EnvLabel};

use super::connection_form::ConnectionForm;
use super::theme;
use crate::model::AppModel;

pub struct ConnectionList {
    model: Entity<AppModel>,
}

impl ConnectionList {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
    }
}

/// 一覧を閉じて、編集フォームのダイアログを開く。`existing` が `None` なら新規。
pub fn open_form(
    model: Entity<AppModel>,
    existing: Option<ConnectionSummary>,
    window: &mut Window,
    cx: &mut App,
) {
    window.close_dialog(cx);
    let title = if existing.is_some() {
        "接続を編集"
    } else {
        "新しい接続"
    };
    let form = cx.new(|cx| ConnectionForm::new(model, existing.as_ref(), window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title(title).w(px(520.)).child(form.clone())
    });
}

pub fn open_list(model: Entity<AppModel>, window: &mut Window, cx: &mut App) {
    let list = cx.new(|cx| ConnectionList::new(model, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("接続先").w(px(620.)).child(list.clone())
    });
}

impl Render for ConnectionList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let connections = self.model.read(cx).connections.clone();
        let rows: Vec<_> = connections
            .into_iter()
            .enumerate()
            .map(|(ix, c)| {
                let (open, edit, del) = (c.clone(), c.clone(), c.id.clone());
                let target = format!(
                    "{}@{}:{}/{}",
                    c.username.clone().unwrap_or_default(),
                    c.host.clone().unwrap_or_default(),
                    c.port.map(|p| p.to_string()).unwrap_or_default(),
                    c.database.clone().unwrap_or_default()
                );
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(c.name.clone()),
                                    )
                                    .when(c.env_label == EnvLabel::Production, |d| {
                                        d.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::production_red())
                                                .child("本番"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(target),
                            ),
                    )
                    .child(
                        Button::new(("conn-open", ix))
                            .label("接続")
                            .primary()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let id = open.id.clone();
                                this.model.update(cx, |m, _| m.open_tab(id));
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new(("conn-edit", ix))
                            .label("編集")
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                open_form(this.model.clone(), Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("conn-del", ix))
                            .label("削除")
                            .small()
                            .danger()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let id = del.clone();
                                this.model.update(cx, |m, _| m.delete_connection(id));
                            })),
                    )
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .when(empty, |d| {
                d.child(
                    div()
                        .p_3()
                        .text_color(theme.muted_foreground)
                        .child("接続先がありません。「新規作成」から追加してください"),
                )
            })
            .child(
                div().flex().flex_row().justify_end().child(
                    Button::new("conn-new")
                        .label("新規作成")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            open_form(this.model.clone(), None, window, cx)
                        })),
                ),
            )
    }
}
