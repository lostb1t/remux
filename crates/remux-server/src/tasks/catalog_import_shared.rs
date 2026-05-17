use anyhow::Result;
use futures::StreamExt;
use std::collections::HashMap;
use tracing::error;
use uuid::Uuid;

use super::ProgressReporter;
use crate::db;

/// Consume `stream`, upserting items and recording catalog membership.
///
/// For addon-sourced catalogs (media_id starting with `addon:`), each top-level
/// imported item gets a row in `media_catalog_items`. Legacy catalogs continue to
/// use `media_relations` with `RelationRole::Catalog`.
///
/// Items already carry their `kind`, IDs, and metadata — no enrichment happens here.
/// Returns a map of `kind -> count` for top-level items imported.
pub async fn import_catalog_items<S>(
    db: &sqlx::SqlitePool,
    catalog_id: Uuid,
    media_id: &str,
    max: usize,
    stream: S,
    progress: &ProgressReporter,
) -> Result<HashMap<String, usize>>
where
    S: futures::Stream<Item = db::Media> + Unpin,
{
    let mut chunks = stream.chunks(500);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    let membership = catalog_membership(media_id);

    while let Some(items) = chunks.next().await {
        progress.report(total, max.max(1));

        let remaining = max.saturating_sub(total);
        if remaining == 0 {
            break;
        }

        let items: Vec<db::Media> = items.into_iter().take(remaining).collect();
        if items.is_empty() {
            break;
        }

        if let Err(e) = db::Media::upsert(db, &items).await {
            error!(catalog = media_id, error = %e, "failed to import chunk");
            continue;
        }

        if let Some((addon_uuid, local_cat_id)) = membership {
            // Note: after upsert the stored `id` may differ from `item.id` when
            // the row was matched by the (kind, media_id) unique index instead of
            // the primary key.  We resolve the real id via a subquery so the FK
            // on media_catalog_items is never violated.
            for item in items.iter().filter(|m| m.parent_id.is_none()) {
                if let Err(e) = sqlx::query(
                    "INSERT OR IGNORE INTO media_catalog_items (media_id, addon_id, catalog_id) \
                     SELECT id, ?1, ?2 FROM media \
                     WHERE CASE WHEN ?3 IS NOT NULL \
                                THEN (kind = ?4 AND media_id = ?3) \
                                ELSE id = ?5 \
                           END \
                     LIMIT 1",
                )
                .bind(addon_uuid)
                .bind(local_cat_id)
                .bind(&item.media_id)
                .bind(&item.kind)
                .bind(item.id)
                .execute(db)
                .await
                {
                    error!(catalog = media_id, error = %e, "failed to record catalog membership");
                }
            }
        }

        for item in items.iter().filter(|m| m.parent_id.is_none()) {
            *counts.entry(item.kind.to_string()).or_insert(0) += 1;
        }
        total = counts.values().sum();
        if total >= max {
            break;
        }
    }

    Ok(counts)
}

/// Extract `(addon_uuid_str, local_catalog_id)` from an addon-sourced `media_id`.
/// Returns `None` for legacy catalog IDs that don't start with `addon:`.
pub fn catalog_membership(media_id: &str) -> Option<(&str, &str)> {
    let rest = media_id.strip_prefix("addon:")?;
    rest.split_once(':')
}
