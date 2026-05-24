use anyhow::Result;
use futures::stream::StreamExt;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::ProgressReporter;
use crate::{AppContext, db};

/// Consume `stream`, fetching metadata + full tree for new items and upserting everything.
///
/// For each chunk from the stream:
/// - Items already in DB with `refreshed_at` set are upserted as-is (basic field update).
/// - New items go through `process_meta_item` which fetches metadata and builds the full
///   tree (seasons, episodes). The tree is upserted in one shot so items appear in the DB
///   with complete data from the start.
///
/// Returns a map of `kind -> count` for top-level items imported.
pub async fn import_catalog_items<S>(
    ctx: &AppContext,
    catalog_id: Uuid,
    media_id: &str,
    max: usize,
    stream: S,
    progress: &ProgressReporter,
) -> Result<HashMap<String, usize>>
where
    S: futures::Stream<Item = db::Media> + Unpin,
{
    let mut chunks = stream.chunks(250);
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

        // Separate top-level items (series/movies) from any sub-items in the stream.
        let top_level: Vec<db::Media> = items
            .iter()
            .filter(|m| m.parent_id.is_none())
            .cloned()
            .collect();
        let top_ids: Vec<Uuid> = top_level.iter().map(|m| m.id).collect();

        // One batch query: which of these are already in DB with metadata?
        let already_refreshed = fetch_already_refreshed_ids(&ctx.db, &top_ids).await;

        let (new_items, existing_items): (Vec<db::Media>, Vec<db::Media>) = top_level
            .into_iter()
            .partition(|m| !already_refreshed.contains(&m.id));

        debug!(
            catalog = media_id,
            new = new_items.len(),
            existing = existing_items.len(),
            "processing chunk"
        );

        // New items: fetch meta + save via process_meta_batch (handles upsert + relations).
        if let Err(e) = ctx.addons.process_meta_batch(new_items, ctx, false).await {
            error!(catalog = media_id, error = %e, "failed to process new items chunk");
            continue;
        }

        // Existing items: already have metadata, just ensure they're current in DB.
        if !existing_items.is_empty() {
            if let Err(e) = db::Media::upsert(&ctx.db, &existing_items).await {
                error!(catalog = media_id, error = %e, "failed to upsert existing chunk");
                continue;
            }
        }

        // Checkpoint the WAL after each chunk so it doesn't balloon to hundreds of MB
        // during large imports, which would make concurrent reads increasingly slow.
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&ctx.db)
            .await
            .ok();

        // Record catalog membership for the original top-level IDs.
        if let Some((addon_uuid, local_cat_id)) = membership {
            for id in &top_ids {
                if let Err(e) = sqlx::query(
                    "INSERT OR IGNORE INTO media_catalog_items (media_id, addon_id, catalog_id) \
                     SELECT id, ?1, ?2 FROM media WHERE id = ?3 LIMIT 1",
                )
                .bind(addon_uuid)
                .bind(local_cat_id)
                .bind(id)
                .execute(&ctx.db)
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

/// Returns the set of IDs from `ids` that are already in the DB with `refreshed_at` set.
async fn fetch_already_refreshed_ids(
    db: &sqlx::SqlitePool,
    ids: &[Uuid],
) -> HashSet<Uuid> {
    if ids.is_empty() {
        return HashSet::new();
    }
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT id FROM media WHERE refreshed_at IS NOT NULL AND id IN (",
    );
    let mut sep = qb.separated(", ");
    for id in ids {
        sep.push_bind(*id);
    }
    qb.push(")");
    let rows: Vec<(Uuid,)> =
        qb.build_query_as().fetch_all(db).await.unwrap_or_default();
    rows.into_iter().map(|(id,)| id).collect()
}

/// Delete rows from `media_catalog_items` whose (addon_id, catalog_id) pair is not in `valid_pairs`.
pub async fn remove_stale_catalog_memberships(
    db: &sqlx::SqlitePool,
    valid_pairs: &HashSet<(String, String)>,
) {
    let existing: Vec<(String, String)> = match sqlx::query_as(
        "SELECT DISTINCT addon_id, catalog_id FROM media_catalog_items",
    )
    .fetch_all(db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to fetch catalog memberships for cleanup");
            return;
        }
    };

    let stale: Vec<&(String, String)> = existing
        .iter()
        .filter(|p| !valid_pairs.contains(*p))
        .collect();

    if stale.is_empty() {
        return;
    }

    info!(count = stale.len(), "removing stale catalog memberships");
    for (addon_id, catalog_id) in stale {
        if let Err(e) = sqlx::query(
            "DELETE FROM media_catalog_items WHERE addon_id = ? AND catalog_id = ?",
        )
        .bind(addon_id)
        .bind(catalog_id)
        .execute(db)
        .await
        {
            warn!(addon = %addon_id, catalog = %catalog_id, error = %e, "failed to remove stale catalog membership");
        }
    }
}

/// Extract `(addon_uuid_str, local_catalog_id)` from an addon-sourced `media_id`.
/// Returns `None` for legacy catalog IDs that don't start with `addon:`.
pub fn catalog_membership(media_id: &str) -> Option<(&str, &str)> {
    let rest = media_id.strip_prefix("addon:")?;
    rest.split_once(':')
}
