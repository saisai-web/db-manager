//! スキーマのイベントを、モデルに適用する。

use sqltool_core::Event;

use super::AppModel;

impl AppModel {
    pub(super) fn apply_schema(&mut self, event: Event) {
        match event {
            Event::Schemas {
                connection_id,
                schemas,
            } => {
                self.prefetch_schemas(&connection_id, &schemas);
                let s = self.schemas.entry(connection_id).or_default();
                s.error = None;
                s.schemas = Some(schemas);
            }
            Event::Tables {
                connection_id,
                schema,
                tables,
            } => {
                self.schemas
                    .entry(connection_id.clone())
                    .or_default()
                    .tables
                    .insert(schema.clone(), tables);
                self.prefetch_columns(&connection_id, &schema);
            }
            Event::TableDetails {
                connection_id,
                details,
            } => {
                let key = (details.table.schema.clone(), details.table.name.clone());
                self.schemas
                    .entry(connection_id)
                    .or_default()
                    .details
                    .insert(key, details);
            }
            Event::SchemaInvalidated { connection_id } => {
                // core はキャッシュを捨て終えている。表示側も捨て、開いている枝を読み直す
                // (ここで RefreshSchema を送ると、core がまた同じ通知を返して終わらない)
                self.schemas
                    .entry(connection_id.clone())
                    .or_default()
                    .invalidate();
                self.reload_expanded(&connection_id);
            }
            _ => {}
        }
    }
}
