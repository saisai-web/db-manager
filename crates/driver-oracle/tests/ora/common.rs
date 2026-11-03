//! 結合テストの共通基盤: 接続設定と、結果を集めるシンク。

use async_trait::async_trait;
use driver_api::{
    ConnectionConfig, Driver, ExecSummary, Flow, ResultColumn, RowSink, Secret, Session, Statement,
    TlsMode,
};
use driver_oracle::OracleDriver;

pub fn config(db: &test_oracle::TestDb) -> ConnectionConfig {
    let mut cfg = ConnectionConfig::new(&db.host);
    cfg.port = Some(db.port);
    cfg.username = Some(db.user.clone());
    cfg.password = Some(Secret::new(db.password.clone()));
    cfg.database = Some(db.service.clone());
    cfg.tls = TlsMode::Disable;
    cfg
}

#[derive(Default)]
pub struct Collect {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<Vec<u8>>>>,
    /// この行数ごとに `Pause` を返す(0 なら返さない)
    pub pause_at: usize,
    /// `resume` が返す値を順に使う。尽きたら Continue
    pub resume_with: Vec<Flow>,
    pub resumed: usize,
    pub keep_rows: bool,
    pub count: usize,
}

impl Collect {
    pub fn keeping() -> Self {
        Self {
            keep_rows: true,
            ..Self::default()
        }
    }

    pub fn text(&self, row: usize, col: usize) -> Option<String> {
        self.rows[row][col]
            .as_ref()
            .map(|b| String::from_utf8(b.clone()).unwrap())
    }
}

#[async_trait]
impl RowSink for Collect {
    fn columns(&mut self, columns: &[ResultColumn]) -> Flow {
        self.columns = columns.to_vec();
        Flow::Continue
    }

    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        self.count += 1;
        if self.keep_rows {
            self.rows
                .push(cells.iter().map(|c| c.map(<[u8]>::to_vec)).collect());
        }
        if self.pause_at != 0 && self.count.is_multiple_of(self.pause_at) {
            return Flow::Pause;
        }
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        self.resumed += 1;
        if self.resume_with.is_empty() {
            Flow::Continue
        } else {
            self.resume_with.remove(0)
        }
    }
}

pub async fn session(db: &test_oracle::TestDb) -> Box<dyn Session> {
    OracleDriver.connect(&config(db)).await.expect("connect")
}

pub async fn run(s: &mut Box<dyn Session>, sql: &str) -> (ExecSummary, Collect) {
    let mut sink = Collect::keeping();
    let summary = s
        .execute(&Statement::new(sql), &mut sink)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    (summary, sink)
}

/// 結果を返さない文を、ただ実行する。
pub async fn exec(s: &mut Box<dyn Session>, sql: &str) -> ExecSummary {
    run(s, sql).await.0
}
