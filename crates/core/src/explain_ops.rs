//! 実行計画の取得。

use std::sync::Arc;

use async_trait::async_trait;
use driver_api::{Flow, PlanFormat, ResultColumn, RowSink, Statement};

use crate::core::{Res, Shared};
use crate::explain::parse_postgres;
use crate::plan::{ExecuteTarget, plan};
use crate::rules::scan_rules;
use crate::types::*;

/// 先頭の 1 行 1 列を、文字列として受け取る。
#[derive(Default)]
struct FirstCell(Option<String>);

#[async_trait]
impl RowSink for FirstCell {
    fn columns(&mut self, _: &[ResultColumn]) -> Flow {
        Flow::Continue
    }

    fn row(&mut self, cells: &[Option<&[u8]>]) -> Flow {
        if self.0.is_none() {
            self.0 = cells
                .first()
                .copied()
                .flatten()
                .map(|b| String::from_utf8_lossy(b).into_owned());
        }
        Flow::Continue
    }

    async fn resume(&mut self) -> Flow {
        Flow::Continue
    }
}

impl Shared {
    pub(crate) async fn explain(
        self: &Arc<Self>,
        tab_id: TabId,
        sql: String,
        target: ExecuteTarget,
        analyze: bool,
    ) -> Res {
        let tab = self.tab(tab_id)?;
        let rules = scan_rules(tab.driver.dialect().lexical_features());
        let statement = plan(&sql, &target, &rules)
            .into_iter()
            .next()
            .ok_or_else(|| UserError::new("実行計画を求める文がありません"))?;
        // ANALYZE は、文を実際に実行する。書き込みを含みうる文には使わせない
        if analyze && !statement.stmt.is_read_only() {
            return Err(UserError::new(
                "実測つきの実行計画は、文を実際に実行します。SELECT など、読み取りだけの文で使ってください",
            ));
        }
        let (explain_sql, format) = tab
            .driver
            .dialect()
            .explain_structured_sql(&statement.text, analyze)
            .ok_or_else(|| UserError::new("このデータベースは、実行計画の表示に未対応です"))?;
        {
            let mut rt = tab.rt();
            if rt.run != RunState::Idle {
                return Err(UserError::new("実行中のため、実行計画を求められません"));
            }
            rt.run = RunState::Running;
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Running,
        });

        let result = {
            let mut guard = tab.session.lock().await;
            match guard.as_mut() {
                Some(session) => {
                    let mut sink = FirstCell::default();
                    session
                        .execute(&Statement::new(explain_sql), &mut sink)
                        .await
                        .map_err(UserError::from)
                        .and_then(|_| sink.0.ok_or_else(|| UserError::new("実行計画が空です")))
                }
                None => Err(UserError::new("接続が切れています。再接続してください")),
            }
        };

        tab.rt().run = RunState::Idle;
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Idle,
        });
        let json = result?;
        let parsed = match format {
            PlanFormat::PostgresJson => parse_postgres(&json),
        }
        .map_err(UserError::new)?;
        self.emit(Event::ExplainReady {
            tab: tab_id,
            sql: statement.text,
            analyze,
            plan: parsed,
        });
        Ok(())
    }
}
