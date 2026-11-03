use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use driver_api::{MetadataProvider, SchemaInfo, TableInfo, TableRef};

use crate::types::{TableDetails, UserError};

pub(crate) type Connector = Box<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Box<dyn MetadataProvider>, UserError>> + Send>>
        + Send
        + Sync,
>;

/// 接続先ごとのスキーマ情報のキャッシュ。メタデータ用の別接続を持つので、
/// 実行中のクエリに待たされない。補完は、ここにあるものだけを使い、入力中に DB へ問い合わせない。
#[derive(Default)]
pub(crate) struct SchemaCache {
    inner: tokio::sync::Mutex<Data>,
}

#[derive(Default)]
struct Data {
    provider: Option<Box<dyn MetadataProvider>>,
    schemas: Option<Vec<SchemaInfo>>,
    tables: HashMap<String, Vec<TableInfo>>,
    details: HashMap<(String, String), TableDetails>,
}

impl Data {
    async fn provider(&mut self, connect: &Connector) -> Result<&dyn MetadataProvider, UserError> {
        if self.provider.is_none() {
            self.provider = Some(connect().await?);
        }
        Ok(self.provider.as_deref().expect("just connected"))
    }

    /// 失敗したら、接続が壊れているかもしれないので、次回は開き直す。
    fn on_error(&mut self, e: driver_api::DriverError) -> UserError {
        self.provider = None;
        e.into()
    }
}

impl SchemaCache {
    pub async fn schemas(&self, connect: &Connector) -> Result<Vec<SchemaInfo>, UserError> {
        let mut d = self.inner.lock().await;
        if let Some(s) = &d.schemas {
            return Ok(s.clone());
        }
        let result = d.provider(connect).await?.schemas().await;
        let schemas = result.map_err(|e| d.on_error(e))?;
        d.schemas = Some(schemas.clone());
        Ok(schemas)
    }

    pub async fn tables(
        &self,
        connect: &Connector,
        schema: &str,
    ) -> Result<Vec<TableInfo>, UserError> {
        let mut d = self.inner.lock().await;
        if let Some(t) = d.tables.get(schema) {
            return Ok(t.clone());
        }
        let result = d.provider(connect).await?.tables(schema).await;
        let tables = result.map_err(|e| d.on_error(e))?;
        d.tables.insert(schema.to_string(), tables.clone());
        Ok(tables)
    }

    pub async fn details(
        &self,
        connect: &Connector,
        table: &TableRef,
    ) -> Result<TableDetails, UserError> {
        let mut d = self.inner.lock().await;
        let key = (table.schema.clone(), table.name.clone());
        if let Some(t) = d.details.get(&key) {
            return Ok(t.clone());
        }
        let provider = d.provider(connect).await?;
        let fetched = async {
            Ok::<_, driver_api::DriverError>(TableDetails {
                table: table.clone(),
                columns: provider.columns(table).await?,
                keys: provider.keys(table).await?,
                indexes: provider.indexes(table).await?,
            })
        }
        .await;
        let details = fetched.map_err(|e| d.on_error(e))?;
        d.details.insert(key, details.clone());
        Ok(details)
    }

    /// キャッシュを捨てる。接続は保つ。
    pub async fn invalidate(&self) {
        let mut d = self.inner.lock().await;
        d.schemas = None;
        d.tables.clear();
        d.details.clear();
    }

    pub async fn close(&self) {
        let mut d = self.inner.lock().await;
        if let Some(p) = d.provider.take() {
            let _ = p.close().await;
        }
    }
}
