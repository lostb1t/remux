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
                // If the item arrived with a resolvable external ID (TMDB or TVDB), we
                // expected to derive an IMDB ID from it. Persisting without one produces
                // a UUID mismatch in validate() and the item gets dropped anyway — bail
                // early so the caller sees a clean failure instead of a silent crash.
                if media
                    .external_ids
                    .tmdb
                    .is_some()
                    || media
                        .external_ids
                        .tvdb
                        .is_some()
                {
                    warn!(%id, kind = ?media.kind, title = %media.title,
                        "persist_from_store: IMDB resolution failed for TMDB/TVDB item, skipping");
                    return Ok(None);
                }
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

        // Already in DB — alias is saved above, skip the full tree sync.
        if let Some(existing) = db::Media::get_by_id(&ctx.db, &resolved_id).await? {
            return Ok(Some(existing));
        }

        let config = std::sync::Arc::new(
            crate::db::Settings::get_config_or_default(&ctx.db).await,
        );
        let processed: Vec<db::Media> = {
            use futures::StreamExt;
            ctx.addons
                .process_meta_item(root, ctx.clone(), false, config)
                .collect()
                .await
        };
        if !processed.is_empty() {
            db::Media::upsert(&ctx.db, &processed)
                .await
                .ok();
            crate::addons::save_pending_relations(ctx, &processed).await;
            let series_ids: Vec<_> = processed
                .iter()
                .filter(|item| item.kind == db::MediaKind::Series)
                .map(|item| item.id)
                .collect();
            db::Media::reconcile_series_identities(&ctx.db, &series_ids)
                .await
                .ok();
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
        // Fast path: already in DB or aliased to a stable UUID in the store.
        if let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? {
            return Ok(Some(media));
        }
        if let Some(real_id) = ctx
            .store
            .get::<Uuid>(id.to_string())
        {
            if let Some(media) = db::Media::get_by_id(&ctx.db, &real_id).await? {
                return Ok(Some(media));
            }
        }

        // Slow path: acquire per-ID lock so only one concurrent request persists.
        let _guard = PERSIST_LOCKS
            .lock(id)
            .await;
        // Re-check after acquiring lock — another request may have persisted it.
        if let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? {
            return Ok(Some(media));
        }
        if let Some(real_id) = ctx
            .store
            .get::<Uuid>(id.to_string())
        {
            if let Some(media) = db::Media::get_by_id(&ctx.db, &real_id).await? {
                return Ok(Some(media));
            }
        }
        Self::persist_from_store(id, ctx).await
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
