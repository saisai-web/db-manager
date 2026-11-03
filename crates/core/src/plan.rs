//! 実行する文の決定、安全確認、トランザクション状態の追跡。DB に依存しない純粋なロジック。

use std::ops::Range;

use sqlscan::{Rules, Statement, StatementKind, split, statement_at};

/// エディタ上の、何を実行するか。位置は UTF-8 のバイト位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteTarget {
    /// カーソル位置の文(Cmd/Ctrl + Enter で、選択範囲がないとき)
    Cursor { offset: usize },
    /// 選択範囲(Cmd/Ctrl + Enter で、選択範囲があるとき)
    Selection { start: usize, end: usize },
    /// エディタ内の全文(Cmd/Ctrl + Shift + Enter)
    All,
}

/// 実行が決まった 1 つの文。
#[derive(Debug, Clone)]
pub struct Planned {
    /// エディタ全体の中での位置
    pub range: Range<usize>,
    /// DB に送る SQL(前後の空白、コメント、終端の `;` を除いたもの)
    pub text: String,
    pub stmt: Statement,
}

/// 実行する文を決める。選択範囲は、その範囲だけを改めて分割する。
pub fn plan(sql: &str, target: &ExecuteTarget, rules: &Rules) -> Vec<Planned> {
    let to_planned = |base: usize, text: &str, st: &Statement| Planned {
        range: base + st.range.start..base + st.range.end,
        text: st.text(text).to_string(),
        stmt: st.clone(),
    };
    match target {
        ExecuteTarget::All => split(sql, rules)
            .iter()
            .map(|st| to_planned(0, sql, st))
            .collect(),
        ExecuteTarget::Cursor { offset } => {
            let all = split(sql, rules);
            let offset = (*offset).min(sql.len());
            statement_at(&all, offset)
                .map(|st| vec![to_planned(0, sql, st)])
                .unwrap_or_default()
        }
        ExecuteTarget::Selection { start, end } => {
            let (start, end) = (*start.min(end), *start.max(end));
            let (start, end) = (start.min(sql.len()), end.min(sql.len()));
            let Some(selected) = sql.get(start..end) else {
                return Vec::new(); // 文字の途中を指している
            };
            split(selected, rules)
                .iter()
                .map(|st| to_planned(start, selected, st))
                .collect()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    /// WHERE のない UPDATE
    UnfilteredUpdate,
    /// WHERE のない DELETE
    UnfilteredDelete,
    Drop,
    Truncate,
}

/// 実行前に利用者へ確認する文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmItem {
    /// 実行する文の中での位置(0 始まり)
    pub statement_index: usize,
    pub kind: ConfirmKind,
    /// 対象が分かる、空白を整えた文の先頭部分
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub statement_index: usize,
    /// エディタ上の、拒否した文の先頭
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Assessment {
    /// 読み取り専用モードで実行できない文。1 つでもあれば、何も実行しない。
    pub refusal: Option<Refusal>,
    pub confirmations: Vec<ConfirmItem>,
}

/// 実行前の安全確認。
///
/// 読み取り専用モードでは、書き込みを含みうる文が 1 つでもあれば全体を拒否する。途中まで
/// 実行して止まるより、何も起きないほうが誤操作に気づきやすいため。
pub fn assess(plan: &[Planned], read_only: bool) -> Assessment {
    let mut out = Assessment::default();
    for (i, p) in plan.iter().enumerate() {
        if read_only && !p.stmt.is_read_only() {
            out.refusal = Some(Refusal {
                statement_index: i,
                offset: p.range.start,
            });
            return out;
        }
        let kind = match p.stmt.kind {
            StatementKind::Update if !p.stmt.has_where => Some(ConfirmKind::UnfilteredUpdate),
            StatementKind::Delete if !p.stmt.has_where => Some(ConfirmKind::UnfilteredDelete),
            StatementKind::Drop => Some(ConfirmKind::Drop),
            StatementKind::Truncate => Some(ConfirmKind::Truncate),
            _ => None,
        };
        if let Some(kind) = kind {
            out.confirmations.push(ConfirmItem {
                statement_index: i,
                kind,
                summary: summarize(&p.text),
            });
        }
    }
    out
}

const SUMMARY_CHARS: usize = 160;

/// 空白を 1 つにまとめ、長ければ切り詰める。
pub fn summarize(sql: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for word in sql.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count == SUMMARY_CHARS {
                out.push('…');
                return out;
            }
            out.push(ch);
            count += 1;
        }
    }
    out
}

/// 実行した文の先頭から数えた文字位置(0 始まり)を、エディタ全体のバイト位置にする。
pub fn editor_offset(planned: &Planned, char_position: usize) -> usize {
    let byte = planned
        .text
        .char_indices()
        .nth(char_position)
        .map_or(planned.text.len(), |(i, _)| i);
    planned.range.start + byte
}

/// トランザクションの状態。PostgreSQL のプロトコルは状態を知らせてくれないため、
/// 実行した文の種類から追跡する。`COMMIT AND CHAIN` のような連鎖は追えず、
/// 次の COMMIT / ROLLBACK で正しくなる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TxnState {
    #[default]
    Idle,
    /// 開始済みで、未コミット
    Open,
    /// エラーで中断した。ROLLBACK まで他の文は実行できない
    Failed,
}

impl TxnState {
    pub fn is_pending(self) -> bool {
        self != Self::Idle
    }

    pub fn after_success(self, st: &Statement) -> Self {
        if st.kind != StatementKind::Transaction {
            if st.keyword.as_deref() == Some("PREPARE")
                && st.second_keyword.as_deref() == Some("TRANSACTION")
            {
                return Self::Idle;
            }
            return self;
        }
        let second = st.second_keyword.as_deref();
        match (st.keyword.as_deref(), second) {
            // 準備済みトランザクションの確定・取り消しは、今のトランザクションとは無関係
            (Some("COMMIT" | "ROLLBACK"), Some("PREPARED")) => self,
            (Some("BEGIN" | "START"), _) => Self::Open,
            (Some("COMMIT" | "END" | "ABORT"), _) => Self::Idle,
            (Some("ROLLBACK"), Some("TO")) => Self::Open,
            (Some("ROLLBACK"), _) => Self::Idle,
            _ => self, // SAVEPOINT / RELEASE
        }
    }

    pub fn after_error(self) -> Self {
        match self {
            Self::Open | Self::Failed => Self::Failed,
            Self::Idle => Self::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg() -> Rules {
        Rules::postgres()
    }

    fn texts(p: &[Planned]) -> Vec<&str> {
        p.iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn all_runs_every_statement_in_order() {
        let sql = "select 1; select 2;\nselect 3";
        let p = plan(sql, &ExecuteTarget::All, &pg());
        assert_eq!(texts(&p), ["select 1", "select 2", "select 3"]);
        assert_eq!(&sql[p[2].range.clone()], "select 3");
    }

    #[test]
    fn cursor_picks_the_statement_under_the_cursor() {
        let sql = "select 1;\nselect 22;\nselect 333";
        let at = |o| plan(sql, &ExecuteTarget::Cursor { offset: o }, &pg());
        assert_eq!(texts(&at(3)), ["select 1"]);
        assert_eq!(texts(&at(12)), ["select 22"]);
        assert_eq!(texts(&at(sql.len())), ["select 333"]);
        assert_eq!(
            texts(&at(10_000)),
            ["select 333"],
            "offset past the end is clamped"
        );
        assert!(plan("  ", &ExecuteTarget::Cursor { offset: 0 }, &pg()).is_empty());
    }

    #[test]
    fn selection_runs_only_the_selected_text_even_if_it_cuts_a_statement() {
        let sql = "select 1; select a, b from t where x = 1; select 3";
        // 「select a, b」だけを選択
        let start = sql.find("select a").unwrap();
        let end = start + "select a, b".len();
        let p = plan(sql, &ExecuteTarget::Selection { start, end }, &pg());
        assert_eq!(texts(&p), ["select a, b"]);
        assert_eq!(&sql[p[0].range.clone()], "select a, b");
    }

    #[test]
    fn selection_with_several_statements_keeps_absolute_ranges() {
        let sql = "-- c\nselect 1; select 2; select 3";
        let start = sql.find("select 2").unwrap();
        let p = plan(
            sql,
            &ExecuteTarget::Selection {
                start,
                end: sql.len(),
            },
            &pg(),
        );
        assert_eq!(texts(&p), ["select 2", "select 3"]);
        for x in &p {
            assert_eq!(&sql[x.range.clone()], x.text);
        }
    }

    #[test]
    fn selection_handles_reversed_out_of_range_and_non_boundary_offsets() {
        let sql = "select '日本語'; select 2";
        let p = plan(
            sql,
            &ExecuteTarget::Selection {
                start: sql.len() + 50,
                end: 0,
            },
            &pg(),
        );
        assert_eq!(p.len(), 2, "reversed and clamped");
        // 多バイト文字の途中を指す選択は、実行しない
        let mid = sql.find('日').unwrap() + 1;
        assert!(plan(sql, &ExecuteTarget::Selection { start: 0, end: mid }, &pg()).is_empty());
        assert!(plan(sql, &ExecuteTarget::Selection { start: 3, end: 3 }, &pg()).is_empty());
    }

    #[test]
    fn read_only_refuses_the_whole_batch_if_any_statement_writes() {
        let sql = "select 1; update t set a = 1 where id = 1; select 2";
        let p = plan(sql, &ExecuteTarget::All, &pg());
        let a = assess(&p, true);
        assert_eq!(
            a.refusal,
            Some(Refusal {
                statement_index: 1,
                offset: sql.find("update").unwrap()
            })
        );
        assert!(assess(&p, false).refusal.is_none());
        let reads = plan(
            "select 1; show all; explain select 2",
            &ExecuteTarget::All,
            &pg(),
        );
        assert!(assess(&reads, true).refusal.is_none());
    }

    #[test]
    fn confirmations_for_unfiltered_dml_and_destructive_ddl() {
        let sql = "delete from t;\nupdate t set a = 1;\ndrop table  public.old_stuff;\n\
                   truncate users, orders;\ndelete from t where id = 1;\nselect 1";
        let a = assess(&plan(sql, &ExecuteTarget::All, &pg()), false);
        let got: Vec<_> = a
            .confirmations
            .iter()
            .map(|c| (c.statement_index, c.kind))
            .collect();
        assert_eq!(
            got,
            [
                (0, ConfirmKind::UnfilteredDelete),
                (1, ConfirmKind::UnfilteredUpdate),
                (2, ConfirmKind::Drop),
                (3, ConfirmKind::Truncate),
            ]
        );
        assert_eq!(
            a.confirmations[2].summary, "drop table public.old_stuff",
            "names the target"
        );
        assert_eq!(a.confirmations[3].summary, "truncate users, orders");
    }

    #[test]
    fn cte_delete_without_where_needs_confirmation() {
        let a = assess(
            &plan(
                "with x as (select 1) delete from t using x",
                &ExecuteTarget::All,
                &pg(),
            ),
            false,
        );
        assert_eq!(a.confirmations[0].kind, ConfirmKind::UnfilteredDelete);
    }

    #[test]
    fn summary_collapses_whitespace_and_truncates_on_char_boundaries() {
        assert_eq!(summarize("  drop \n\t table   x  "), "drop table x");
        let long = format!("drop table {}", "あ".repeat(500));
        let s = summarize(&long);
        assert_eq!(s.chars().count(), SUMMARY_CHARS + 1);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn error_position_maps_to_the_editor_offset() {
        let sql = "select 1;\nselect '日本語', from t";
        let p = plan(sql, &ExecuteTarget::Cursor { offset: sql.len() }, &pg());
        // DB は「from」の位置を、文の先頭から数えた文字数で返す
        let char_pos = p[0].text[..p[0].text.find("from").unwrap()].chars().count();
        let off = editor_offset(&p[0], char_pos);
        assert_eq!(&sql[off..off + 4], "from");
        assert_eq!(
            editor_offset(&p[0], 10_000),
            p[0].range.end,
            "clamped to the end"
        );
    }

    fn st(sql: &str) -> Statement {
        split(sql, &pg()).remove(0)
    }

    #[test]
    fn transaction_state_follows_statements() {
        let s = TxnState::Idle;
        assert_eq!(s.after_success(&st("select 1")), TxnState::Idle);
        let s = s.after_success(&st("begin"));
        assert_eq!(s, TxnState::Open);
        assert_eq!(
            s.after_success(&st("insert into t values (1)")),
            TxnState::Open
        );
        assert_eq!(s.after_success(&st("savepoint a")), TxnState::Open);
        assert_eq!(s.after_success(&st("commit")), TxnState::Idle);
        assert_eq!(s.after_success(&st("end")), TxnState::Idle);
        assert_eq!(s.after_success(&st("rollback")), TxnState::Idle);
        assert_eq!(s.after_success(&st("abort")), TxnState::Idle);
        assert_eq!(
            TxnState::Idle.after_success(&st("start transaction read only")),
            TxnState::Open
        );
    }

    #[test]
    fn rollback_to_savepoint_keeps_or_restores_the_transaction() {
        assert_eq!(
            TxnState::Open.after_success(&st("rollback to savepoint a")),
            TxnState::Open
        );
        // 中断したトランザクションは、セーブポイントまで戻せば再開できる
        assert_eq!(
            TxnState::Failed.after_success(&st("rollback to a")),
            TxnState::Open
        );
    }

    #[test]
    fn prepared_transaction_commands_do_not_touch_the_current_transaction() {
        assert_eq!(
            TxnState::Open.after_success(&st("commit prepared 'x'")),
            TxnState::Open
        );
        assert_eq!(
            TxnState::Idle.after_success(&st("rollback prepared 'x'")),
            TxnState::Idle
        );
        assert_eq!(
            TxnState::Open.after_success(&st("prepare transaction 'x'")),
            TxnState::Idle
        );
    }

    #[test]
    fn errors_abort_an_open_transaction_only() {
        assert_eq!(TxnState::Idle.after_error(), TxnState::Idle);
        assert_eq!(TxnState::Open.after_error(), TxnState::Failed);
        assert_eq!(TxnState::Failed.after_error(), TxnState::Failed);
        // 中断中の COMMIT は、PostgreSQL では ROLLBACK として成功する
        assert_eq!(
            TxnState::Failed.after_success(&st("commit")),
            TxnState::Idle
        );
        assert!(TxnState::Failed.is_pending() && TxnState::Open.is_pending());
        assert!(!TxnState::Idle.is_pending());
    }
}
