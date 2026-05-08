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
/// imported item gets tagged with `catalog:{addon_uuid}:{local_id}`. Smart
/// collections filter on those tags. Legacy catalogs continue to use
/// `media_relations` with `RelationRole::Catalog`.
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
    let catalog_tag = catalog_membership_tag(media_id);

    while let Some(items) = chunks.next().await {
        progress.set(total as f64 / max.max(1) as f64 * 100.0);

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

        if let Some(tag) = catalog_tag.as_deref() {
            // Addon-sourced catalogs use tags to track membership.
            for item in items.iter().filter(|m| m.parent_id.is_none()) {
                if let Err(e) = sqlx::query(
                    "INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)",
                )
                .bind(item.id)
                .bind(tag)
                .execute(db)
                .await
                {
                    error!(catalog = media_id, error = %e, "failed to tag item");
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

/// Compute the membership tag for a catalog's `media_id`. Addon-sourced
/// catalogs return `Some("catalog:{addon_uuid}:{local_id}")`; legacy
/// catalogs return `None`.
pub fn catalog_membership_tag(media_id: &str) -> Option<String> {
    let rest = media_id.strip_prefix("addon:")?;
    let (uuid_str, local_id) = rest.split_once(':')?;
    Some(format!("catalog:{uuid_str}:{local_id}"))
}
