//! テーブル編集の操作: テーブルを開く、変更のプレビュー、承認後の保存。

use std::sync::Arc;

use driver_api::{Session, Statement, TableKind};

use crate::core::{Res, Shared, lock};
use crate::edit::{EditAccess, PlannedEdit, TableSource, plan as plan_edit};
use crate::plan::ExecuteTarget;
use crate::plan::TxnState;
use crate::tabs::NullSink;
use crate::types::*;

/// プレビュー済みで、承認を待っている編集。
pub(crate) struct PendingEdit {
    pub tab: TabId,
    pub statements: Vec<PlannedEdit>,
}

impl Shared {
    /// スキーマツリーのテーブルを全行で開く。キーがあれば、結果は編集できる。
    pub(crate) async fn open_table(self: &Arc<Self>, tab_id: TabId, table: TableRef) -> Res {
        let tab = self.tab(tab_id)?;
        let connection = tab.connection.id.clone();
        let cache = self.schema_cache(&connection);
        let connect = self.connector(connection);
        let details = cache.details(&connect, &table).await?;
        let kind = cache
            .tables(&connect, &table.schema)
            .await?
            .iter()
            .find(|t| t.name == table.name)
            .map_or(TableKind::Table, |t| t.kind);
        let access = TableSource::access(&details, kind);
        let sql = tab.driver.dialect().select_table_sql(&table, None);
        self.execute(
            tab_id,
            sql,
            ExecuteTarget::All,
            ExecuteOptions::default(),
            access,
        )
        .await
    }

    /// 溜めた変更から SQL を作り、プレビューとして返す。DB には何も送らない。
    pub(crate) fn prepare_edit(
        self: &Arc<Self>,
        tab_id: TabId,
        result: ResultId,
        changes: Vec<crate::edit::Change>,
    ) -> Res {
        let tab = self.tab(tab_id)?;
        {
            let rt = tab.rt();
            if rt.read_only {
                return Err(UserError::new("読み取り専用モードでは保存できません"));
            }
            if rt.txn.is_pending() {
                return Err(UserError::new(
                    "未コミットのトランザクションがあります。COMMIT か ROLLBACK してから保存してください",
                ));
            }
        }
        let handle = self.result_handle(result)?;
        let EditAccess::Editable(source) = handle.edit_access() else {
            return Err(UserError::new("この結果は編集できません"));
        };
        let statements = plan_edit(source, &changes, tab.driver.dialect())
            .map_err(|e| UserError::new(e.to_string()))?;
        let items = statements
            .iter()
            .map(|s| EditPreviewItem {
                kind: s.kind,
                sql: s.preview.clone(),
            })
            .collect();
        let plan = PlanId(self.id());
        lock(&self.edit_plans).insert(
            plan,
            PendingEdit {
                tab: tab_id,
                statements,
            },
        );
        self.emit(Event::EditPreview {
            tab: tab_id,
            plan,
            statements: items,
        });
        Ok(())
    }

    /// プレビューを承認して保存する。全体が 1 つのトランザクションで、どれかの影響行数が
    /// 1 でなければ、すべて取り消す。
    pub(crate) async fn commit_edit(self: &Arc<Self>, tab_id: TabId, plan: PlanId) -> Res {
        let tab = self.tab(tab_id)?;
        let pending = {
            let mut plans = lock(&self.edit_plans);
            match plans.get(&plan) {
                Some(p) if p.tab == tab_id => plans.remove(&plan),
                _ => None,
            }
        }
        .ok_or_else(|| {
            UserError::new("保存する変更が見つかりません。もう一度、保存してください")
        })?;
        {
            let mut rt = tab.rt();
            if rt.run != RunState::Idle {
                return Err(UserError::new("実行中のため、保存できません"));
            }
            if rt.txn != TxnState::Idle {
                return Err(UserError::new("未コミットのトランザクションがあります"));
            }
            rt.run = RunState::Running;
        }
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Running,
        });

        let outcome = {
            let mut guard = tab.session.lock().await;
            match guard.as_mut() {
                Some(session) => {
                    let d = tab.driver.dialect();
                    let r = run_in_transaction(
                        session.as_mut(),
                        &pending.statements,
                        (d.begin_sql(), d.commit_sql(), d.rollback_sql()),
                    )
                    .await;
                    if matches!(&r, Err(EditRejection::Database { error, .. }) if error.kind == ErrorKind::Disconnected)
                    {
                        guard.take();
                        self.on_disconnected(&tab);
                    }
                    r
                }
                None => Err(EditRejection::Database {
                    statement_index: 0,
                    error: UserError {
                        kind: ErrorKind::Disconnected,
                        ..UserError::new("接続が切れています。再接続してください")
                    },
                }),
            }
        };

        tab.rt().run = RunState::Idle;
        self.emit(Event::RunStateChanged {
            tab: tab_id,
            state: RunState::Idle,
        });
        self.emit(match outcome {
            Ok(()) => Event::EditCommitted {
                tab: tab_id,
                plan,
                statements: pending.statements.len(),
            },
            Err(reason) => Event::EditRejected {
                tab: tab_id,
                plan,
                reason,
            },
        });
        Ok(())
    }
}

/// 1 つのトランザクションで、すべての文を実行する。影響行数が 1 でなければ取り消す。
async fn run_in_transaction(
    session: &mut dyn Session,
    statements: &[PlannedEdit],
    (begin, commit, rollback): (&str, &str, &str),
) -> Result<(), EditRejection> {
    let db = |i: usize, e: driver_api::DriverError| EditRejection::Database {
        statement_index: i,
        error: e.into(),
    };
    session
        .execute(&Statement::new(begin), &mut NullSink)
        .await
        .map_err(|e| db(0, e))?;
    for (i, st) in statements.iter().enumerate() {
        let stmt = Statement::with_params(st.sql.clone(), st.params.clone());
        let result = session.execute(&stmt, &mut NullSink).await;
        let rejection = match result {
            Ok(sum) if sum.affected_rows == Some(1) => continue,
            Ok(sum) => EditRejection::RowCount {
                statement_index: i,
                actual: sum.affected_rows.unwrap_or(0),
                sql: st.preview.clone(),
            },
            Err(e) => db(i, e),
        };
        // 取り消しに失敗しても、元の理由を返す(接続が切れていれば、DB 側でも取り消される)
        let _ = session
            .execute(&Statement::new(rollback), &mut NullSink)
            .await;
        return Err(rejection);
    }
    session
        .execute(&Statement::new(commit), &mut NullSink)
        .await
        .map_err(|e| db(statements.len(), e))?;
    Ok(())
}
