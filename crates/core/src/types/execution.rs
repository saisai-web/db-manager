//! 実行の型: 取得上限、状態、結果、終了状態。

use std::time::Duration;

use super::{CsvOptions, ResultId, UserError};
use crate::plan::TxnState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchLimit {
    /// 既定(10 万行)で一時停止する
    #[default]
    Default,
    Rows(u64),
    /// 一時停止しない
    Unlimited,
}

#[derive(Debug, Clone, Default)]
pub struct ExecuteOptions {
    pub fetch_limit: FetchLimit,
    /// 文ごとのタイムアウト。超えるとサーバーに停止を依頼する
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMore {
    /// 次の 1 回分(取得上限と同じ行数)
    NextBatch,
    /// 残りをすべて
    Rest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    /// 実行の前に、利用者の確認を待っている
    WaitingConfirmation,
    Running,
    /// 取得上限で受信を止めている。接続は占有されたまま
    Paused {
        result: ResultId,
    },
    /// 停止を依頼した。サーバー側の完了は待たない
    Cancelling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncateReason {
    /// 利用者が打ち切った
    UserStopped,
    /// 結果のメモリ上限に達した(一時ファイルへの退避が無効)
    MemoryLimit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementOutcome {
    Completed {
        rows: u64,
        /// DB が返した行数。DDL では 0
        affected_rows: Option<u64>,
        elapsed: Duration,
        has_result_set: bool,
    },
    /// 結果の一部だけを受信して止めた。並べ替えはできない
    Truncated {
        rows: u64,
        elapsed: Duration,
        reason: TruncateReason,
    },
    Failed {
        error: UserError,
        /// エディタ全体でのバイト位置。DB が位置を示した場合
        editor_offset: Option<usize>,
        elapsed: Duration,
    },
    Cancelled {
        elapsed: Duration,
    },
    TimedOut {
        elapsed: Duration,
        limit: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    Completed,
    /// 文のどれかが失敗して止めた。残りは実行していない
    Failed {
        statement_index: usize,
    },
    Cancelled,
    TimedOut {
        statement_index: usize,
    },
    /// 読み取り専用モードで実行できない文があるため、何も実行しなかった
    RefusedReadOnly {
        statement_index: usize,
        /// エディタ全体での、拒否した文の先頭(バイト位置)
        editor_offset: usize,
    },
    /// 確認が承諾されなかったため、何も実行しなかった
    Declined,
    /// 実行する文がなかった
    NothingToRun,
}

#[derive(Debug, Clone)]
pub enum ExportFormat {
    Csv(CsvOptions),
    Xlsx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseBlock {
    /// 未コミットのトランザクションがある
    PendingTransaction(TxnState),
    Running,
}
