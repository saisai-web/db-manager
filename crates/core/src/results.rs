//! 結果の並べ替えと、結果の検索。

use std::sync::Arc;

use crate::core::{Res, Shared};
use crate::result::ResultHandle;
use crate::types::*;

impl Shared {
    pub(crate) async fn sort_result(
        self: &Arc<Self>,
        id: ResultId,
        column: usize,
        order: SortOrder,
    ) -> Res {
        let handle = self.result_handle(id)?;
        if !handle.can_sort() {
            return Err(UserError::new("全件を受信するまで並べ替えられません"));
        }
        let kind = handle
            .sort_kind(column)
            .ok_or_else(|| UserError::new("列が範囲外です"))?;
        let store = handle.store().clone();
        let index = tokio::task::spawn_blocking(move || store.sort_index(column, order, kind))
            .await
            .map_err(|e| UserError::new(e.to_string()))?
            .map_err(|e| UserError::new(e.to_string()))?;
        handle.set_sorted(column, order, index);
        self.emit(Event::SortChanged { result: id });
        Ok(())
    }

    pub(crate) fn result_handle(&self, id: ResultId) -> Res<ResultHandle> {
        self.results()
            .get(&id)
            .cloned()
            .ok_or_else(|| UserError::new("結果が見つかりません"))
    }
}
