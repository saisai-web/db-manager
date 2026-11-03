//! スキーマ情報の取得(メタデータ用の別接続とキャッシュ)。

use std::sync::Arc;

use crate::core::{Shared, lock};
use crate::schema::{Connector, SchemaCache};
use crate::types::*;

impl Shared {
    pub(crate) fn schema_cache(&self, connection_id: &str) -> Arc<SchemaCache> {
        lock(&self.schemas)
            .entry(connection_id.to_string())
            .or_default()
            .clone()
    }

    /// メタデータ用の別接続を開く関数。
    pub(crate) fn connector(self: &Arc<Self>, connection_id: ConnectionId) -> Connector {
        let sh = Arc::clone(self);
        Box::new(move || {
            let sh = Arc::clone(&sh);
            let id = connection_id.clone();
            Box::pin(async move {
                let rec = sh.record(&id).await?;
                let driver = sh.driver(&rec.driver)?;
                let cfg = sh.connection_config(&rec).await?;
                Ok(driver.connect_metadata(&cfg).await?)
            })
        })
    }
}
