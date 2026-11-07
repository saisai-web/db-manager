//! Query入力。結果の上に出る SQL の窓の開閉と、複数のタブ。
//! 右クリックで作った SQL や、「100 行を表示」の SQL は、新しいタブに入る(書きかけを消さない)。

use gpui_kit::component::input::{EditorState, InputContent};
use gpui_kit::{AppContext as _, Context, Entity, Focusable as _, Window};
use sqltool_core::{Command, ConnectionSummary, SELECT_PREVIEW_ROWS, SqlKind, TabId, TableRef};

use super::AppModel;
use super::tab::QueryDoc;

fn kind_label(kind: SqlKind) -> &'static str {
    match kind {
        SqlKind::Select => "SELECT",
        SqlKind::SelectAll => "SELECT 全件",
        SqlKind::Insert => "INSERT",
        SqlKind::Update => "UPDATE",
        SqlKind::Delete => "DELETE",
    }
}

impl AppModel {
    /// SQL 用のエディタ(色分け・行番号・補完つき)を作る。
    pub(super) fn new_query_editor(
        &mut self,
        connection: &ConnectionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorState> {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
                .searchable(true)
                .placeholder("SQL を入力し、Cmd/Ctrl + Enter で実行")
        });
        let provider = crate::ui::SqlCompletion {
            model: cx.entity().downgrade(),
            connection: connection.id.clone(),
            driver: connection.driver.clone(),
        };
        editor.update(cx, |e, _| {
            e.lsp_mut().completion_provider = Some(std::rc::Rc::new(provider));
        });
        editor
    }

    pub fn open_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query_open = true;
        if let Some(t) = self.active_tab() {
            let focus = t.editor().read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub fn close_query(&mut self, cx: &mut Context<Self>) {
        self.query_open = false;
        cx.notify();
    }

    /// 新しい Query タブを足して選ぶ。`title` が無ければ「クエリ N」、`sql` があれば最初から入れる。
    pub fn add_query(
        &mut self,
        tab: TabId,
        title: Option<String>,
        sql: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.tab(tab) else { return };
        let connection = t.connection.clone();
        let editor = self.new_query_editor(&connection, window, cx);
        if let Some(sql) = sql {
            editor.update(cx, |e, cx| e.set_value(InputContent::from(sql), window, cx));
        }
        let Some(t) = self.tab_mut(tab) else { return };
        let title = title.unwrap_or_else(|| format!("クエリ {}", t.next_query_no));
        t.next_query_no += 1;
        t.queries.push(QueryDoc {
            title: title.into(),
            editor,
        });
        t.current_query = t.queries.len() - 1;
        self.active = Some(tab);
        self.open_query(window, cx);
    }

    pub fn select_query(
        &mut self,
        tab: TabId,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(t) = self.tab_mut(tab) {
            t.current_query = ix.min(t.queries.len() - 1);
        }
        self.open_query(window, cx);
    }

    /// Query タブを閉じる。最後の 1 つは閉じられない。
    pub fn close_query_doc(&mut self, tab: TabId, ix: usize, cx: &mut Context<Self>) {
        let Some(t) = self.tab_mut(tab) else { return };
        if t.queries.len() <= 1 || ix >= t.queries.len() {
            return;
        }
        t.queries.remove(ix);
        if t.current_query > ix || t.current_query >= t.queries.len() {
            t.current_query = t.current_query.saturating_sub(1);
        }
        cx.notify();
    }

    /// テーブルを開き、同じ内容の SELECT を、Query入力の新しいタブに出す。
    /// `limit` が `Some` なら先頭の行だけ、`None` なら全件。
    pub fn view_table(&mut self, tab: TabId, table: TableRef, limit: Option<u64>) {
        let kind = if limit.is_some() {
            debug_assert_eq!(limit, Some(SELECT_PREVIEW_ROWS));
            SqlKind::Select
        } else {
            SqlKind::SelectAll
        };
        self.open_table(tab, table.clone(), limit);
        self.request_sql(tab, table, kind);
    }

    /// テーブルの SQL の雛形を頼む。届いたら、新しいタブに入れる。
    pub fn request_sql(&mut self, tab: TabId, table: TableRef, kind: SqlKind) {
        self.core.send(Command::GenerateSql { tab, table, kind });
    }

    pub(super) fn on_sql_generated(
        &mut self,
        tab: TabId,
        table: &TableRef,
        kind: SqlKind,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = format!("{} {}", table.name, kind_label(kind));
        self.add_query(tab, Some(title), Some(sql.to_string()), window, cx);
    }
}
