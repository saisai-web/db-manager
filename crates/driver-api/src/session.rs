use async_trait::async_trait;

use crate::{ConnectionConfig, Dialect, MetadataProvider, PlanFormat, Result};

/// ドライバが対応する機能。`core` と UI は DB 名ではなく、これで分岐する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub schemas: bool,
    /// 実行中のクエリを別タスクから止められる
    pub cancel: bool,
    pub read_only_session: bool,
    /// DDL もトランザクションで取り消せる
    pub transactional_ddl: bool,
    /// UPDATE / DELETE / INSERT の影響行数が取得できる
    pub affected_rows: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// `PostgreSQL 16.2 ...` など、サーバーが名乗る版
    pub version: String,
}

#[async_trait]
pub trait Driver: Send + Sync {
    fn capabilities(&self) -> &Capabilities;
    fn dialect(&self) -> &dyn Dialect;
    /// タブ 1 つぶんのセッションを開く。
    async fn connect(&self, cfg: &ConnectionConfig) -> Result<Box<dyn Session>>;
    /// スキーマ情報の取得用に、セッションとは別の接続を開く。
    async fn connect_metadata(&self, cfg: &ConnectionConfig) -> Result<Box<dyn MetadataProvider>>;
}

/// 実行する 1 つの文。分割は呼び出し側(sqlscan)が済ませている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub sql: String,
    /// バインドする値(`None` が NULL)。値は文字列で渡し、SQL 側で型に変換する
    /// ([`Dialect::bind_text`](crate::Dialect::bind_text))。空なら、バインドなしで実行する。
    pub params: Vec<Option<String>>,
}

impl Statement {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            params: Vec::new(),
        }
    }

    pub fn with_params(sql: impl Into<String>, params: Vec<Option<String>>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

/// 値の大まかな分類。画面の寄せ方、ソートの比較方法、バイナリの表示を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueClass {
    Text,
    /// 整数、numeric、浮動小数点。文字列のまま 10 進数として比較する
    Number,
    Bool,
    /// 日付、時刻、timestamp、interval。DB が返した文字列のまま持つ
    Temporal,
    /// バイト列で保持する。画面には 16 進数の先頭だけ出す
    Binary,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultColumn {
    pub name: String,
    /// 型名。分からない場合は `None`
    pub type_name: Option<String>,
    pub class: ValueClass,
}

/// 受信した行の行き先を、ドライバへ返す指示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// 受信を一時停止する。ドライバは [`RowSink::resume`] を待つ。
    /// 待つ間は接続が占有され、サーバーからの送信は TCP の流量制御で止まる。
    Pause,
    /// 受信を打ち切る。ドライバは残りを読み捨てず、クエリを止める。
    Stop,
}

/// 結果の受け取り側。行ごとの `async` による確保を避けるため `row` は同期で、
/// 一時停止のときだけ `resume` を待つ。
#[async_trait]
pub trait RowSink: Send {
    /// 結果セットの列が決まったとき、行より先に 1 回呼ばれる。
    /// 行を返さない文(INSERT など)では呼ばれない。
    fn columns(&mut self, columns: &[ResultColumn]) -> Flow;

    /// 1 行。`None` が NULL(空文字列とは別)。バイナリ列は復号済みのバイト列。
    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow;

    /// `Flow::Pause` を返した後に呼ばれ、続行か打ち切りが決まるまで待つ。
    /// `Flow::Pause` を返してはならない。
    async fn resume(&mut self) -> Flow;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecSummary {
    /// `RowSink::row` に渡した行数。シンクが受け取らず `Flow::Stop` を返した行も含むため、
    /// 結果に保持した行数は、シンク側で数えること。
    pub rows_received: u64,
    /// DB が完了時に返した行数。INSERT / UPDATE / DELETE では影響行数、結果セットを返した文では
    /// その行数(打ち切った場合は不明で `None`)。DDL などでは 0。
    /// 結果セットを返さない文かどうかは、`RowSink::columns` が呼ばれたかで区別する。
    pub affected_rows: Option<u64>,
    /// `Flow::Stop` で受信を打ち切った
    pub stopped_early: bool,
}

/// 実行中のクエリを別タスクから止める。
#[async_trait]
pub trait Canceller: Send + Sync {
    /// サーバーに停止を依頼する。結果が返るまでの完了は待たなくてよい。
    async fn cancel(&self) -> Result<()>;
}

/// エディタタブ 1 つに割り当てる接続。トランザクションはセッション単位。
#[async_trait]
pub trait Session: Send {
    fn server_info(&self) -> &ServerInfo;

    /// 文を 1 つ実行し、結果を `sink` へ流す。
    async fn execute(&mut self, stmt: &Statement, sink: &mut dyn RowSink) -> Result<ExecSummary>;

    /// 構造化された実行計画(文字列と、その形式)を、セッション自身が求める。サーバーの種類や版で
    /// 文が変わる DB 用。対応しなければ `None` を返し、呼び出し側は
    /// [`Dialect::explain_structured_sql`](crate::Dialect::explain_structured_sql) を使う。
    async fn explain(
        &mut self,
        _sql: &str,
        _analyze: bool,
    ) -> Result<Option<(String, PlanFormat)>> {
        Ok(None)
    }

    /// セッションの読み取り専用モードを、セッション自身が切り替える。[`Dialect::set_read_only_sql`]
    /// の SQL で切り替えられない DB 用(SQL Server と Oracle は、文ごとのトランザクションで実現する)。
    async fn set_read_only(&mut self, _read_only: bool) -> Result<()> {
        Ok(())
    }

    /// 実行中でも別タスクから呼べる。
    fn canceller(&self) -> Box<dyn Canceller>;

    async fn close(self: Box<Self>) -> Result<()>;
}
