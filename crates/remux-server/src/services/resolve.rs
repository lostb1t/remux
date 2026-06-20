use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use axum_anyhow::{ApiError, ApiResult as Result};
use http::StatusCode;
use remux_sdks::{RestClient, deezer as dz};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{AppContext, AppState, db, keyed_lock::KeyedLock};

pub struct MediaResolveService;

impl MediaResolveService {
    async fn resolve_media_imdb(media: &mut db::Media, ctx: &AppContext) -> bool {
        if media
            .external_ids
            .imdb
            .is_some()
        {
            return true;
        }
        let is_tv = matches!(media.kind, db::MediaKind::Series);
        let Some(client) = crate::common::tmdb_client(
            &ctx.db,
            &ctx.config
                .tmdb_base_url,
        )
        .await
        else {
            return false;
        };
        let Some(imdb) = crate::addons::tmdb::resolve_imdb_from_ids(
            &media.external_ids,
            is_tv,
            &client,
        )
        .await
        else {
            return false;
        };
        media
            .external_ids
            .imdb = db::NonEmptyString::try_new(imdb).ok();
        true
    }

    async fn resolve_music_deezer(media: &mut db::Media) -> bool {
        match media.kind {
            db::MediaKind::Track => {
                if media
                    .external_ids
                    .deezer_track
                    .is_some()
                {
                    return true;
                }
                let Ok(client) = RestClient::new("https://api.deezer.com/") else {
                    return false;
                };
                let hit = match client
                    .execute(dz::SearchTracksEndpoint {
                        q: media
                            .title
                            .clone(),
                        limit: 1,
                    })
                    .await
                {
                    Ok(dz::DeezerResult::Ok(list)) => list
                        .data
                        .into_iter()
                        .next(),
                    Ok(dz::DeezerResult::Err { error }) => {
                        warn!(title = %media.title, %error, "Deezer track search returned error");
                        return false;
                    }
                    Err(e) => {
                        warn!(title = %media.title, error = %e, "Deezer track search HTTP error");
                        return false;
                    }
                };
                let Some(track) = hit else { return false };
                media
                    .external_ids
                    .deezer_track = Some(track.id as i64);
                media
                    .external_ids
                    .deezer_album = Some(
                    track
                        .album
                        .id as i64,
                );
                media
                    .external_ids
                    .deezer_artist = Some(
                    track
                        .artist
                        .id as i64,
                );
                true
            }
            db::MediaKind::Album => {
                if media
                    .external_ids
                    .deezer_album
                    .is_some()
                {
                    return true;
                }
                let Ok(client) = RestClient::new("https://api.deezer.com/") else {
                    return false;
                };
                let hit = match client
                    .execute(dz::SearchAlbumsEndpoint {
                        q: media
                            .title
                            .clone(),
                        limit: 1,
                    })
                    .await
                {
                    Ok(dz::DeezerResult::Ok(list)) => list
                        .data
                        .into_iter()
                        .next(),
                    Ok(dz::DeezerResult::Err { error }) => {
                        warn!(title = %media.title, %error, "Deezer album search returned error");
                        return false;
                    }
                    Err(e) => {
                        warn!(title = %media.title, error = %e, "Deezer album search HTTP error");
                        return false;
                    }
                };
                let Some(album) = hit else { return false };
                media
                    .external_ids
                    .deezer_album = Some(album.id as i64);
                media
                    .external_ids
                    .deezer_artist = Some(
                    album
                        .artist
                        .id as i64,
                );
                true
            }
            _ => false,
        }
    }

    /// Resolves a cached search result from the store into a persisted `db::Media`.
    ///
    /// - Movie/Series: resolves IMDB ID first (via TMDB), then saves.
    /// - Track/Album: builds artist root from `external_ids.deezer_artist`, runs
    ///   `process_meta_item` which triggers `sync_tree` → full discography.
    /// - Artist/Person: passed directly to `process_meta_item`.
    async fn persist_from_store(
        id: Uuid,
        ctx: &AppContext,
    ) -> anyhow::Result<Option<db::Media>> {
        let Some(mut media) = ctx
            .store
            .get::<db::Media>(id.to_string())
        else {
            return Ok(None);
        };
        ctx.store
            .delete(id.to_string());

        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
            if !Self::resolve_media_imdb(&mut media, ctx).await {
                warn!(%id, kind = ?media.kind, "persist_from_store: IMDB resolution failed, saving without IMDB ID");
            }
            // Recompute the stable UUID now that we have the IMDB ID. Use the authoritative
            // path (media_id_raw → From<MediaIdRaw>) which correctly handles all kinds.
            if media
                .external_ids
                .imdb
                .is_some()
            {
                media.id = uuid::Uuid::from(&media.media_id_raw());
            }
        }

        if matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album) {
            if !Self::resolve_music_deezer(&mut media).await {
                warn!(%id, kind = ?media.kind, title = %media.title,
                    "persist_from_store: Deezer ID resolution failed");
            }
        }

        let root = if matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
        {
            let Some(deezer_artist_id) = media
                .external_ids
                .deezer_artist
            else {
                debug!(%id, kind = ?media.kind, "persist_from_store: no deezer_artist id on music child");
                return Ok(None);
            };
            db::Media {
                id: crate::common::stable_media_uuid(
                    &db::MediaKind::Artist,
                    &deezer_artist_id.to_string(),
                ),
                title: media
                    .grandparent
                    .as_ref()
                    .map(|gp| {
                        gp.title
                            .clone()
                    })
                    .unwrap_or_default(),
                kind: db::MediaKind::Artist,
                external_ids: db::ExternalIds {
                    deezer_artist: Some(deezer_artist_id),
                    ..Default::default()
                },
                ..Default::default()
            }
        } else {
            media
        };

        // Save the (possibly recomputed stable) ID before root is consumed by process_meta_item.
        let resolved_id = root.id;

        // If the caller's fake UUID differs from the resolved real UUID, keep an alias so
        // future lookups for the fake ID still resolve to the persisted row.
        if id != resolved_id {
            ctx.store
                .save(
                    id.to_string(),
                    resolved_id,
                    std::time::Duration::from_secs(7 * 24 * 3600),
                );
        }

        let config = std::sync::Arc::new(
            crate::db::Settings::get_config(&ctx.db)
                .await
                .unwrap_or_default(),
        );
        let processed = ctx
            .addons
            .process_meta_item(root, ctx, false, config)
            .await;
        if !processed.is_empty() {
            db::Media::upsert(&ctx.db, &processed)
                .await
                .ok();
            crate::addons::save_pending_relations(ctx, &processed).await;
        }
        Ok(db::Media::get_by_id(&ctx.db, &resolved_id).await?)
    }

    /// For each candidate ID: if not in DB, acquire its persist lock and persist if still missing;
    /// if in DB but lock is held, wait for it. Returns true if a query retry is warranted.
    pub(crate) async fn wait_for_persist(
        ids: &[Uuid],
        ctx: &AppContext,
    ) -> anyhow::Result<bool> {
        for &id in ids {
            let in_db = db::Media::get_by_id(&ctx.db, &id)
                .await?
                .is_some();
            if !in_db {
                let _guard = PERSIST_LOCKS
                    .lock(id)
                    .await;
                if db::Media::get_by_id(&ctx.db, &id)
                    .await?
                    .is_none()
                {
                    Self::persist_from_store(id, ctx)
                        .await
                        .ok();
                }
                return Ok(true);
            } else if let Some(_guard) = PERSIST_LOCKS
                .lock_if_exists(&id)
                .await
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Resolves a synthetic item ID to a real `db::Media` row, persisting it via addons if needed.
    ///
    /// Uses a per-ID mutex to prevent duplicate persists from concurrent requests.
    pub(crate) async fn resolve_item(
        id: Uuid,
        ctx: &AppContext,
    ) -> anyhow::Result<Option<db::Media>> {
        resolve_item_core(
            id,
            || async {
                if let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? {
                    return Ok(Some(media));
                }
                if let Some(real_id) = ctx
                    .store
                    .get::<Uuid>(id.to_string())
                {
                    return Ok(db::Media::get_by_id(&ctx.db, &real_id).await?);
                }
                Ok(None)
            },
            || Self::persist_from_store(id, ctx),
        )
        .await
    }

    /// Resolves a batch of possibly-transient UUIDs to their stable persisted IDs.
    /// Uses `media.id` from the resolved item (not the input ID) since `persist_from_store`
    /// may recompute a stable UUID from external IDs. Unresolvable IDs are skipped.
    pub(crate) async fn resolve_ids(ids: &[Uuid], ctx: &AppContext) -> Vec<Uuid> {
        let mut resolved = Vec::with_capacity(ids.len());
        for &id in ids {
            match Self::resolve_item(id, ctx).await {
                Ok(Some(media)) => resolved.push(media.id),
                Ok(None) => {
                    warn!(%id, "resolve_ids: could not resolve item, skipping")
                }
                Err(e) => {
                    warn!(%id, err = %e, "resolve_ids: error resolving item, skipping")
                }
            }
        }
        resolved
    }
}

static PERSIST_LOCKS: KeyedLock<Uuid> = KeyedLock::new();

/// Core locking logic, injectable for testing.
///
/// `lookup` — check whether the item is already persisted.
/// `persist` — do the expensive addon persist; called at most once per ID across all concurrent
///             callers.
async fn resolve_item_core<L, P, LFut, PFut>(
    id: Uuid,
    lookup: L,
    persist: P,
) -> anyhow::Result<Option<db::Media>>
where
    L: Fn() -> LFut,
    LFut: std::future::Future<Output = anyhow::Result<Option<db::Media>>>,
    P: Fn() -> PFut,
    PFut: std::future::Future<Output = anyhow::Result<Option<db::Media>>>,
{
    if let Some(media) = lookup().await? {
        return Ok(Some(media));
    }

    let result = {
        let _guard = PERSIST_LOCKS
            .lock(id)
            .await;
        if let Some(media) = lookup().await? {
            Ok(Some(media))
        } else {
            persist().await
        }
        // _guard dropped here — waiters unblock and hit the re-check above
    };

    result
}

/// Axum extractor that resolves the `{id}` path parameter to a persisted `db::Media` row.
///
/// Returns 404 if the ID cannot be resolved even after attempting addon persistence.
pub(crate) struct ResolvedItem(pub db::Media);

impl FromRequestParts<AppState> for ResolvedItem {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Path(id) = Path::<Uuid>::from_request_parts(parts, state)
            .await
            .map_err(|_| {
                ApiError::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .title("Bad Request")
                    .detail("invalid item id")
                    .build()
            })?;

        let media = MediaResolveService::resolve_item(id, &state.ctx)
            .await
            .map_err(|e| {
                ApiError::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .title("Internal Error")
                    .detail("failed to resolve item")
                    .error(e)
                    .build()
            })?
            .ok_or_else(|| {
                ApiError::builder()
                    .status(StatusCode::NOT_FOUND)
                    .title("Not Found")
                    .detail("item not found")
                    .build()
            })?;

        Ok(ResolvedItem(media))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Barrier;

    fn make_media() -> db::Media {
        db::Media {
            id: Uuid::new_v4(),
            title: "Test".into(),
            ..Default::default()
        }
    }

    // --- fast path ---

    #[tokio::test]
    async fn fast_path_returns_existing() {
        let id = Uuid::new_v4();
        let media = make_media();
        let m = media.clone();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let pc = persist_calls.clone();

        let result = resolve_item_core(
            id,
            move || {
                let m = m.clone();
                async move { Ok(Some(m)) }
            },
            move || {
                pc.fetch_add(1, Ordering::SeqCst);
                async { Ok(None) }
            },
        )
        .await
        .unwrap();

        assert_eq!(
            result
                .unwrap()
                .id,
            media.id
        );
        assert_eq!(
            persist_calls.load(Ordering::SeqCst),
            0,
            "persist must not be called"
        );
    }

    // --- slow path: item not in DB, persist saves it ---

    #[tokio::test]
    async fn slow_path_calls_persist_once() {
        let id = Uuid::new_v4();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let pc = persist_calls.clone();
        let media = make_media();
        let m = media.clone();

        let result = resolve_item_core(
            id,
            || async { Ok(None) },
            move || {
                pc.fetch_add(1, Ordering::SeqCst);
                let m = m.clone();
                async move { Ok(Some(m)) }
            },
        )
        .await
        .unwrap();

        assert_eq!(
            result
                .unwrap()
                .id,
            media.id
        );
        assert_eq!(persist_calls.load(Ordering::SeqCst), 1);
    }

    // --- slow path: persist returns None ---

    #[tokio::test]
    async fn slow_path_propagates_none() {
        let id = Uuid::new_v4();
        let result =
            resolve_item_core(id, || async { Ok(None) }, || async { Ok(None) })
                .await
                .unwrap();
        assert!(result.is_none());
    }

    // --- error path: lock is always cleaned up ---

    #[tokio::test]
    async fn error_path_cleans_up_lock() {
        let id = Uuid::new_v4();
        let _ = resolve_item_core(
            id,
            || async { Ok(None) },
            || async { Err(anyhow::anyhow!("persist failed")) },
        )
        .await;

        assert!(
            !PERSIST_LOCKS.contains_key(&id),
            "lock must be removed even after error"
        );
    }

    // --- concurrent: second waiter must not call persist ---

    #[tokio::test]
    async fn concurrent_second_waiter_skips_persist() {
        let id = Uuid::new_v4();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let saved = Arc::new(tokio::sync::RwLock::new(false));

        // Barrier lets both tasks start at the same time.
        let barrier = Arc::new(Barrier::new(2));

        let (pc1, saved1, b1) = (persist_calls.clone(), saved.clone(), barrier.clone());
        let t1 = tokio::spawn(async move {
            b1.wait()
                .await;
            resolve_item_core(
                id,
                {
                    let saved = saved1.clone();
                    move || {
                        let s = saved.clone();
                        async move {
                            Ok(
                                if *s
                                    .read()
                                    .await
                                {
                                    Some(make_media())
                                } else {
                                    None
                                },
                            )
                        }
                    }
                },
                move || {
                    pc1.fetch_add(1, Ordering::SeqCst);
                    let saved = saved1.clone();
                    async move {
                        // Simulate slow persist.
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        *saved
                            .write()
                            .await = true;
                        Ok(Some(make_media()))
                    }
                },
            )
            .await
        });

        let (pc2, saved2, b2) = (persist_calls.clone(), saved.clone(), barrier.clone());
        let t2 = tokio::spawn(async move {
            b2.wait()
                .await;
            resolve_item_core(
                id,
                {
                    let saved = saved2.clone();
                    move || {
                        let s = saved.clone();
                        async move {
                            Ok(
                                if *s
                                    .read()
                                    .await
                                {
                                    Some(make_media())
                                } else {
                                    None
                                },
                            )
                        }
                    }
                },
                move || {
                    pc2.fetch_add(1, Ordering::SeqCst);
                    let saved = saved2.clone();
                    async move {
                        *saved
                            .write()
                            .await = true;
                        Ok(Some(make_media()))
                    }
                },
            )
            .await
        });

        let (r1, r2) = tokio::join!(t1, t2);
        assert!(
            r1.unwrap()
                .unwrap()
                .is_some(),
            "t1 must resolve"
        );
        assert!(
            r2.unwrap()
                .unwrap()
                .is_some(),
            "t2 must resolve"
        );
        assert_eq!(
            persist_calls.load(Ordering::SeqCst),
            1,
            "persist must be called exactly once across both concurrent requests"
        );
    }
}
