use anyhow::Result;
use chrono::NaiveDateTime;
use futures::stream::StreamExt;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, rss_mb};
use crate::{AppContext, addons::ResolvedCatalog, db};

/// Consume `stream`, fetching metadata + full tree for new items and upserting everything.
///
/// Returns a map of `kind -> count` for top-level items imported.
pub async fn import_catalog_items<S>(
    ctx: &AppContext,
    _catalog: &ResolvedCatalog,
    media_id: &str,
    max: usize,
    stream: S,
    progress: &ProgressReporter,
) -> Result<(HashMap<String, usize>, HashMap<String, usize>)>
where
    S: futures::Stream<Item = db::Media> + Unpin,
{
    // Stop pulling from the underlying paginated stream once we have enough items.
    // Without this, the stream fetches ALL pages until empty even when max=50.
    let mut chunks = stream
        .take(max)
        .chunks(250);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut new_counts: HashMap<String, usize> = HashMap::new();
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
                    | db::MediaKind::Track
                    | db::MediaKind::Playlist
            )
        });

        // A playlist carries its tracks in `relations`, which take(max) above
        // can't reach — truncate to honor max_items.
        for item in items.iter_mut() {
            if item.kind == db::MediaKind::Playlist {
                if let Some(rels) = item
                    .relations
                    .as_mut()
                {
                    if rels.len() > max {
                        rels.truncate(max);
                    }
                }
            }
        }

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

        debug!(
            "[mem] catalog {} chunk total={} new={} existing={}: {}MB RSS",
            media_id,
            total + new_items.len() + existing_items.len(),
            new_items.len(),
            existing_items.len(),
            rss_mb()
        );
        if let Err(e) = ctx
            .addons
            .process_meta_batch(new_items.clone(), ctx, true)
            .await
        {
            error!(catalog = media_id, error = %e, "failed to process new items chunk");
            continue;
        }
        debug!(
            "[mem] catalog {} after process_meta_batch: {}MB RSS",
            media_id,
            rss_mb()
        );

        for id in new_series_ids {
            db::reconcile_series_played_state(&ctx.db, id).await;
        }

        // Re-upsert already-known TV channels too (skipping the metadata-fetch step
        // `process_meta_batch` does for new items — a no-op for this kind anyway).
        // Without this, a channel's `updated_at` is frozen from its first import, so
        // the stale-channel prune in `prune_stale_iptv_channels` would treat every
        // still-present channel as stale and delete it. Other kinds (movie/series/
        // etc.) deliberately skip this to avoid re-running their (real) metadata fetch.
        let existing_tv_channels: Vec<db::Media> = existing_items
            .iter()
            .filter(|m| m.kind == db::MediaKind::TvChannel)
            .cloned()
            .collect();
        if !existing_tv_channels.is_empty() {
            if let Err(e) = db::Media::upsert(&ctx.db, &existing_tv_channels).await {
                warn!(catalog = media_id, error = %e, "failed to refresh existing tv channels");
            }
        }

        // Rebuild existing playlist track memberships each refresh.
        let existing_playlists: Vec<db::Media> = existing_items
            .iter()
            .filter(|m| m.kind == db::MediaKind::Playlist)
            .cloned()
            .collect();
        if !existing_playlists.is_empty() {
            if let Err(e) = ctx
                .addons
                .process_meta_batch(existing_playlists, ctx, false)
                .await
            {
                warn!(
                    catalog = media_id,
                    error = %e,
                    "failed to refresh existing playlists"
                );
            }
        }

        // Checkpoint the WAL after each chunk so it doesn't balloon to hundreds of MB
        // during large imports, which would make concurrent reads increasingly slow.
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&ctx.db)
            .await
            .ok();

        // Record catalog membership and apply catalog tags for all top-level IDs.
        // Upsert for both new and existing items to keep positions accurate on re-import.
        //
        // Batched (rather than one INSERT per item/tag): on large catalogs (tens of
        // thousands of items) a per-row loop fires that many individual autocommit
        // writes back-to-back, which starves other connections of SQLite's
        // single-writer lock — a busy connection that immediately re-requests the
        // lock after releasing it tends to keep winning the race over a connection
        // that just started waiting, so unrelated writes (e.g. login) can time out
        // even though no single statement holds the lock for long.
        if collection_id != Uuid::nil() {
            let catalog_tags: Vec<String> =
                if let Some((addon_uuid, local_cat_id)) = membership {
                    ctx.addons
                        .catalog_tags(addon_uuid, local_cat_id)
                } else {
                    vec![]
                };

            let relation_rows: Vec<(Uuid, Uuid, i64, Uuid)> = new_items
                .iter()
                .chain(existing_items.iter())
                .map(|item| {
                    let relation_id = Uuid::new_v5(
                        &collection_id,
                        item.id
                            .as_bytes(),
                    );
                    let weight = catalog_position;
                    catalog_position += 1;
                    (relation_id, collection_id, weight, item.id)
                })
                .collect();

            // 4 bind params/row; chunk well under SQLite's ~999 bound-parameter limit.
            const RELATION_CHUNK: usize = 200;
            for rows in relation_rows.chunks(RELATION_CHUNK) {
                debug!(catalog_id = %collection_id, count = rows.len(), "batch-inserting catalog relations");
                // SQLite doesn't support naming columns of a VALUES table-value
                // constructor (`AS v(a, b)`); its anonymous columns are referenced
                // as column1, column2, ... in binding order.
                let mut qb = sqlx::QueryBuilder::new(
                    "INSERT INTO media_relations (relation_id, left_media_id, right_media_id, role, weight) \
                     SELECT v.column1, v.column2, media.id, 'catalog', v.column3 FROM (",
                );
                qb.push_values(
                    rows.iter(),
                    |mut b, (relation_id, collection_id, weight, item_id)| {
                        b.push_bind(relation_id)
                            .push_bind(collection_id)
                            .push_bind(weight)
                            .push_bind(item_id);
                    },
                );
                qb.push(
                    ") AS v \
                     JOIN media ON media.id = v.column4 \
                     ON CONFLICT (left_media_id, right_media_id, COALESCE(role, '')) DO UPDATE SET weight = excluded.weight",
                );
                if let Err(e) = qb
                    .build()
                    .execute(&ctx.db)
                    .await
                {
                    error!(catalog = media_id, error = %e, "failed to record catalog membership batch");
                }
            }

            if !catalog_tags.is_empty() {
                let tag_rows: Vec<(Uuid, &String)> = new_items
                    .iter()
                    .chain(existing_items.iter())
                    .flat_map(|item| {
                        catalog_tags
                            .iter()
                            .map(move |tag| (item.id, tag))
                    })
                    .collect();

                // 2 bind params/row; chunk well under SQLite's ~999 bound-parameter limit.
                const TAG_CHUNK: usize = 400;
                for rows in tag_rows.chunks(TAG_CHUNK) {
                    let mut qb = sqlx::QueryBuilder::new(
                        "INSERT OR IGNORE INTO media_tags (media_id, tag) ",
                    );
                    qb.push_values(rows.iter(), |mut b, (item_id, tag)| {
                        b.push_bind(item_id)
                            .push_bind(*tag);
                    });
                    if let Err(e) = qb
                        .build()
                        .execute(&ctx.db)
                        .await
                    {
                        warn!(catalog = media_id, error = %e, "failed to apply catalog tags batch");
                    }
                }
            }
        }

        for item in new_items.iter() {
            *new_counts
                .entry(
                    item.kind
                        .to_string(),
                )
                .or_insert(0) += 1;
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

    Ok((counts, new_counts))
}

/// Delete rows from `media_relations` (role='catalog') whose left_media_id is in
/// `domain_collection_ids` (every catalog — enabled or disabled — belonging to the
/// addons this caller is responsible for) but not in `valid_collection_ids` (the
/// catalogs actually imported this run).
///
/// Scoping by `domain_collection_ids` is required because callers (e.g.
/// `RefreshLibraryTask` and `RefreshIptvTask`) each only import a subset of catalog
/// addons; without scoping, one task's cleanup pass would delete the other's
/// still-valid catalog memberships simply because they're absent from its own
/// partial `valid_collection_ids` set. There's no FK/column linking a collection's
/// synthetic id (`Uuid::new_v5(addon_id, local_catalog_id)`) back to its owning addon
/// in the DB, so the scope has to be computed by the caller and passed in.
pub async fn remove_stale_catalog_memberships(
    db: &sqlx::SqlitePool,
    valid_collection_ids: &HashSet<Uuid>,
    domain_collection_ids: &HashSet<Uuid>,
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
        .filter(|id| {
            domain_collection_ids.contains(*id) && !valid_collection_ids.contains(*id)
        })
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

/// Delete `Playlist` media whose addon catalog left `valid_collection_ids`.
/// Run BEFORE `remove_stale_catalog_memberships` — it needs the catalog
/// membership row to locate the orphans.
pub async fn prune_orphaned_playlists(
    db: &sqlx::SqlitePool,
    valid_collection_ids: &HashSet<Uuid>,
    domain_collection_ids: &HashSet<Uuid>,
) {
    let stale: Vec<Uuid> = domain_collection_ids
        .iter()
        .filter(|id| !valid_collection_ids.contains(*id))
        .copied()
        .collect();
    if stale.is_empty() {
        return;
    }

    const STALE_CHUNK: usize = 500;
    let mut orphan_ids: Vec<Uuid> = Vec::new();
    for chunk in stale.chunks(STALE_CHUNK) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT DISTINCT m.id FROM media m \
             JOIN media_relations r ON r.right_media_id = m.id \
             WHERE r.role = 'catalog' AND m.kind = 'playlist' AND r.left_media_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        match qb
            .build_query_scalar::<Uuid>()
            .fetch_all(db)
            .await
        {
            Ok(v) => orphan_ids.extend(v),
            Err(e) => {
                warn!(error = %e, "failed to find orphaned playlists for cleanup");
                return;
            }
        }
    }
    if orphan_ids.is_empty() {
        return;
    }

    info!(count = orphan_ids.len(), "pruning orphaned addon playlists");
    for id in &orphan_ids {
        // Track memberships hang off `left_media_id` (no FK), so drop them
        // explicitly; the catalog membership (left = collection_id) is cleared
        // by remove_stale_catalog_memberships below.
        if let Err(e) = db::MediaRelation::delete_by_left_id(db, id).await {
            warn!(playlist = %id, error = %e, "failed to delete playlist relations");
        }
        if let Err(e) = db::Media::delete(db, id).await {
            warn!(playlist = %id, error = %e, "failed to delete playlist media");
        }
    }
}

pub async fn prune_stale_iptv_channels(db: &sqlx::SqlitePool, cutoff: NaiveDateTime) {
    let mut qb = sqlx::QueryBuilder::new(
        "DELETE FROM media WHERE kind = 'tv_channel' AND updated_at < ",
    );
    qb.push_bind(cutoff);

    match qb
        .build()
        .execute(db)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => {
            info!(count = res.rows_affected(), "pruned stale IPTV channels");
        }
        Ok(_) => {}
        Err(e) => warn!(error = %e, "failed to prune stale IPTV channels"),
    }
}

/// Extract `(addon_uuid_str, local_catalog_id)` from an addon-sourced `media_id`.
/// Returns `None` for legacy catalog IDs that don't start with `addon:`.
pub fn catalog_membership(media_id: &str) -> Option<(&str, &str)> {
    let rest = media_id.strip_prefix("addon:")?;
    rest.split_once(':')
}
