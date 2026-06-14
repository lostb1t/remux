use anyhow::Result;
use futures::stream::StreamExt;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::ProgressReporter;
use crate::{AppContext, addons::CatalogInfo, db};

/// Consume `stream`, fetching metadata + full tree for new items and upserting everything.
///
/// Returns a map of `kind -> count` for top-level items imported.
pub async fn import_catalog_items<S>(
    ctx: &AppContext,
    _catalog: &CatalogInfo,
    media_id: &str,
    max: usize,
    stream: S,
    progress: &ProgressReporter,
) -> Result<HashMap<String, usize>>
where
    S: futures::Stream<Item = db::Media> + Unpin,
{
    // Stop pulling from the underlying paginated stream once we have enough items.
    // Without this, the stream fetches ALL pages until empty even when max=50.
    let mut chunks = stream
        .take(max)
        .chunks(250);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total = 0usize;
    let mut catalog_position = 0i64;
    let membership = catalog_membership(media_id);

    let collection_id: Uuid = match membership {
        Some((addon_str, local_id)) => Uuid::parse_str(addon_str)
            .map(|addon_uuid| Uuid::new_v5(&addon_uuid, local_id.as_bytes()))
            .unwrap_or(Uuid::nil()),
        None => Uuid::nil(),
    };

    while let Some(items) = chunks
        .next()
        .await
    {
        progress.report(total, max.max(1));

        let remaining = max.saturating_sub(total);
        if remaining == 0 {
            break;
        }

        let mut items: Vec<db::Media> = items
            .into_iter()
            .take(remaining)
            .collect();
        if items.is_empty() {
            break;
        }

        // Only process top-level content kinds.
        items.retain(|m| {
            matches!(
                m.kind,
                db::MediaKind::Movie
                    | db::MediaKind::Series
                    | db::MediaKind::Artist
                    | db::MediaKind::TvChannel
                    | db::MediaKind::Album
            )
        });

        // Partition: only new items need meta-batch processing.
        let existing_ids: HashSet<Uuid> = if items.is_empty() {
            HashSet::new()
        } else {
            let mut qb = sqlx::QueryBuilder::new("SELECT id FROM media WHERE id IN (");
            let mut sep = qb.separated(", ");
            for m in &items {
                sep.push_bind(m.id);
            }
            qb.push(")");
            qb.build_query_scalar()
                .fetch_all(&ctx.db)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect()
        };
        let (new_items, existing_items): (Vec<db::Media>, Vec<db::Media>) = items
            .into_iter()
            .partition(|m| !existing_ids.contains(&m.id));

        debug!(
            catalog = media_id,
            new = new_items.len(),
            existing = existing_items.len(),
            "processing chunk"
        );

        let new_series_ids: Vec<Uuid> = new_items
            .iter()
            .filter(|m| m.kind == db::MediaKind::Series)
            .map(|m| m.id)
            .collect();

        if let Err(e) = ctx
            .addons
            .process_meta_batch(new_items.clone(), ctx, false)
            .await
        {
            error!(catalog = media_id, error = %e, "failed to process new items chunk");
            continue;
        }

        for id in new_series_ids {
            db::reconcile_series_played_state(&ctx.db, id).await;
        }

        // Checkpoint the WAL after each chunk so it doesn't balloon to hundreds of MB
        // during large imports, which would make concurrent reads increasingly slow.
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&ctx.db)
            .await
            .ok();

        // Record catalog membership and apply catalog tags for all top-level IDs.
        // Upsert for both new and existing items to keep positions accurate on re-import.
        if collection_id != Uuid::nil() {
            let catalog_tags: Vec<String> =
                if let Some((addon_uuid, local_cat_id)) = membership {
                    ctx.addons
                        .catalog_tags(addon_uuid, local_cat_id)
                } else {
                    vec![]
                };

            for item in new_items
                .iter()
                .chain(existing_items.iter())
            {
                let relation_id = Uuid::new_v5(
                    &collection_id,
                    item.id
                        .as_bytes(),
                );
                debug!(catalog_id = %collection_id, item_id = %item.id, weight = catalog_position, "inserting catalog relation");
                if let Err(e) = sqlx::query(
                    "INSERT INTO media_relations (relation_id, left_media_id, right_media_id, role, weight) \
                     SELECT ?, ?, id, 'catalog', ? FROM media WHERE id = ? \
                     ON CONFLICT (left_media_id, right_media_id, COALESCE(role, '')) DO UPDATE SET weight = excluded.weight",
                )
                .bind(relation_id)
                .bind(collection_id)
                .bind(catalog_position)
                .bind(item.id)
                .execute(&ctx.db)
                .await
                {
                    error!(catalog = media_id, error = %e, "failed to record catalog membership");
                }
                catalog_position += 1;

                for tag in &catalog_tags {
                    if let Err(e) = sqlx::query(
                        "INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)",
                    )
                    .bind(item.id)
                    .bind(tag)
                    .execute(&ctx.db)
                    .await
                    {
                        warn!(catalog = media_id, tag = %tag, error = %e, "failed to apply catalog tag");
                    }
                }
            }
        }

        for item in new_items
            .iter()
            .chain(existing_items.iter())
        {
            *counts
                .entry(
                    item.kind
                        .to_string(),
                )
                .or_insert(0) += 1;
        }
        total = counts
            .values()
            .sum();
        if total >= max {
            break;
        }
    }

    Ok(counts)
}

/// Delete rows from `media_relations` (role='catalog') whose left_media_id is not in `valid_collection_ids`.
pub async fn remove_stale_catalog_memberships(
    db: &sqlx::SqlitePool,
    valid_collection_ids: &HashSet<Uuid>,
) {
    let existing: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT DISTINCT left_media_id FROM media_relations WHERE role = 'catalog'",
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

    let stale: Vec<&Uuid> = existing
        .iter()
        .filter(|id| !valid_collection_ids.contains(*id))
        .collect();

    if stale.is_empty() {
        return;
    }

    info!(count = stale.len(), "removing stale catalog memberships");
    for collection_id in stale {
        if let Err(e) = sqlx::query(
            "DELETE FROM media_relations WHERE left_media_id = ? AND role = 'catalog'",
        )
        .bind(collection_id)
        .execute(db)
        .await
        {
            warn!(collection = %collection_id, error = %e, "failed to remove stale catalog membership");
        }
    }
}

/// Extract `(addon_uuid_str, local_catalog_id)` from an addon-sourced `media_id`.
/// Returns `None` for legacy catalog IDs that don't start with `addon:`.
pub fn catalog_membership(media_id: &str) -> Option<(&str, &str)> {
    let rest = media_id.strip_prefix("addon:")?;
    rest.split_once(':')
}
