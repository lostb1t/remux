//! STUB — filled in by task 4 (payload building).
//!
//! Only the shapes the dispatcher needs exist here. No enrichment is performed
//! yet, so no webhook currently sees item data.

use super::events::WebhookEvent;
use crate::{AppContext, db};
use serde_json::{Map, Value};

/// The library item an event is about, resolved once per event.
pub(crate) struct ItemContext {
    pub media: db::Media,
}

/// TODO(task 4): resolve the item (and its parents) for item-scoped events.
pub(crate) async fn enrich_item(
    _ctx: &AppContext,
    _event: &WebhookEvent,
) -> Option<ItemContext> {
    None
}

/// TODO(task 4): the full Jellyfin-plugin variable set.
pub(crate) fn build_data(
    _ctx: &AppContext,
    event: &WebhookEvent,
    _item: Option<&ItemContext>,
) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert(
        "NotificationType".to_string(),
        Value::String(
            event
                .notification_type()
                .to_string(),
        ),
    );
    data
}
