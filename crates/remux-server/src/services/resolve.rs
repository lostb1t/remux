use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use axum_anyhow::{ApiError, ApiResult as Result};
use http::StatusCode;
use remux_sdks::{BearerAuth, RestClient, deezer as dz};
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    AppContext, AppState, db,
    keyed_lock::KeyedLock,
    sdks,
    sdks::{CachedEndpoint, OptionalEndpoint},
};

/// A mapping that exists never changes, unlike the metadata the addon caches.
const ID_CACHE_TTL: Duration = Duration::from_secs(86400);

/// Not indexed yet is only true until TMDB indexes it, so a new release must
/// not inherit `ID_CACHE_TTL`. Still long enough that one scan asks once.
const ID_MISS_CACHE_TTL: Duration = Duration::from_secs(360);

/// Both arms: a tvdb id TMDB holds as a movie is a real answer even though the
/// series caller gets `None` from it. Only an empty response is a miss.
fn find_matched_nothing(found: &sdks::tmdb::FindByIdResponse) -> bool {
    found
        .movie_results
        .is_empty()
        && found
            .tv_results
            .is_empty()
}

/// The detail calls an id lookup makes, asking for `external_ids` alone.
///
/// The addon fetches the same two paths for metadata on a six minute TTL. The
/// response cache is keyed on the url, so asking for the addon's wider default
/// would land on its entry and stamp `ID_CACHE_TTL` over it, leaving metadata
/// stale for a day.
fn series_ids_endpoint(tmdb_id: i64) -> sdks::tmdb::SeriesEndpoint {
    sdks::tmdb::SeriesEndpoint {
        id: tmdb_id,
        language: None,
        append_to_response: vec!["external_ids".to_string()],
    }
}

fn movie_ids_endpoint(tmdb_id: i64) -> sdks::tmdb::MovieEndpoint {
    sdks::tmdb::MovieEndpoint {
        id: tmdb_id,
        language: None,
        append_to_response: vec!["external_ids".to_string()],
    }
}

pub struct MediaResolveService;

impl MediaResolveService {
    /// `None` only when `tmdb_base_url` will not parse; a missing key falls back
    /// to the bundled one. Reads the settings row, so callers pass it down.
    async fn tmdb(ctx: &AppContext) -> Option<RestClient<BearerAuth>> {
        crate::common::tmdb_client(
            &ctx.db,
            &ctx.config
                .tmdb_base_url,
        )
        .await
    }

    async fn resolve_media_imdb(media: &mut db::Media, ctx: &AppContext) -> bool {
        if media
            .external_ids
            .imdb
            .is_some()
        {
            return true;
        }
        let is_tv = matches!(media.kind, db::MediaKind::Series);
        let Some(client) = Self::tmdb(ctx).await else {
            return false;
        };
        let Some(imdb) =
            Self::resolve_imdb_from_ids(&media.external_ids, is_tv, &client).await
        else {
            return false;
        };
        media
            .external_ids
            .imdb = db::NonEmptyString::try_new(imdb).ok();
        true
    }

    /// From ids alone, never a title search: imdb, then tmdb, then `/find`.
    pub(crate) async fn resolve_imdb_from_ids<A: sdks::Auth + Clone>(
        ids: &db::ExternalIds,
        is_tv: bool,
        client: &RestClient<A>,
    ) -> Option<db::NonEmptyString> {
        if let Some(ref imdb) = ids.imdb {
            return Some(imdb.clone());
        }

        if let Some(tmdb_id) = ids.tmdb {
            if is_tv {
                match client
                    .execute(series_ids_endpoint(tmdb_id).with_cache(ID_CACHE_TTL))
                    .await
                {
                    Ok(series) => {
                        if let Some(imdb) = series
                            .external_ids
                            .and_then(|e| e.imdb_id)
                            .and_then(|s| db::NonEmptyString::try_new(s).ok())
                        {
                            return Some(imdb);
                        }
                        debug!(tmdb_id, "TMDB series has no imdb_id in external_ids");
                    }
                    Err(e) => warn!(tmdb_id, error = %e, "TMDB series lookup failed"),
                }
            } else {
                match client
                    .execute(movie_ids_endpoint(tmdb_id).with_cache(ID_CACHE_TTL))
                    .await
                {
                    Ok(movie) => {
                        if let Some(imdb) = movie
                            .imdb_id
                            .and_then(|s| db::NonEmptyString::try_new(s).ok())
                        {
                            return Some(imdb);
                        }
                        debug!(tmdb_id, "TMDB movie has no imdb_id");
                    }
                    Err(e) => warn!(tmdb_id, error = %e, "TMDB movie lookup failed"),
                }
            }
        }

        // imdb returned above, so this only ever resolves a tvdb id.
        let (external_id, external_source) =
            Self::tmdb_search_key(ids, Some(&sdks::kitsu::client())).await?;
        let tmdb_id =
            Self::find_tmdb_id_by(external_id, external_source, is_tv, client)
                .await
                .ok()
                .flatten()?;

        // FindById returns a partial object without external_ids; use the TMDB id
        // to fetch the full record which includes external_ids (via append_to_response).
        if is_tv {
            let series = client
                .execute(series_ids_endpoint(tmdb_id).with_cache(ID_CACHE_TTL))
                .await
                .ok()?;
            series
                .external_ids
                .and_then(|e| e.imdb_id)
                .and_then(|s| db::NonEmptyString::try_new(s).ok())
        } else {
            let movie = client
                .execute(movie_ids_endpoint(tmdb_id).with_cache(ID_CACHE_TTL))
                .await
                .ok()?;
            movie
                .imdb_id
                .and_then(|s| db::NonEmptyString::try_new(s).ok())
        }
    }

    /// The series' TMDB id as already stored, for an episode or a season: the
    /// preloaded `grandparent` if there is one, else the row `grandparent_id`
    /// names.
    ///
    /// The kind filter is what stops a season being passed off as the series,
    /// whether by a corrupt `grandparent_id` or by a `grandparent` that
    /// `preload_parents` filled from `parent_id`.
    ///
    /// Reads only; [`Self::fill_series_tmdb`] is what resolves and stores.
    pub(crate) async fn stored_series_tmdb_id(
        media: &db::Media,
        ctx: &AppContext,
    ) -> anyhow::Result<Option<i64>> {
        if let Some(tmdb) = media
            .grandparent
            .as_ref()
            .filter(|g| g.kind == db::MediaKind::Series)
            .and_then(|g| {
                g.external_ids
                    .tmdb
            })
        {
            return Ok(Some(tmdb));
        }
        let Some(series_id) = media.grandparent_id else {
            return Ok(None);
        };
        Ok(db::Media::get_by_id(&ctx.db, &series_id)
            .await?
            .filter(|m| m.kind == db::MediaKind::Series)
            .and_then(|m| {
                m.external_ids
                    .tmdb
            }))
    }

    async fn kitsu_tvdb_id(kitsu_id: i64, client: &RestClient) -> Option<i64> {
        match client
            .execute(sdks::kitsu::MappingsEndpoint { kitsu_id })
            .await
        {
            Ok(m) => {
                let tvdb_id = m.tvdb_id();
                debug!(kitsu_id, tvdb_id, "kitsu → tvdb");
                tvdb_id
            }
            Err(e) => {
                warn!(kitsu_id, error = %e, "kitsu mappings lookup failed");
                None
            }
        }
    }

    /// The likeliest id to hit on `/find`: imdb, then tvdb, then tvdb via kitsu.
    /// Not tmdb, which a caller holding one would have skipped this for.
    pub(crate) async fn tmdb_search_key(
        ids: &db::ExternalIds,
        kitsu: Option<&RestClient>,
    ) -> Option<(String, &'static str)> {
        if let Some(ref imdb) = ids.imdb {
            return Some((
                imdb.clone()
                    .into(),
                "imdb_id",
            ));
        }
        if let Some(tvdb) = ids.tvdb {
            return Some((tvdb.to_string(), "tvdb_id"));
        }
        if let (Some(kitsu_id), Some(kitsu_client)) = (ids.kitsu, kitsu) {
            if let Some(tvdb) = Self::kitsu_tvdb_id(kitsu_id, kitsu_client).await {
                return Some((tvdb.to_string(), "tvdb_id"));
            }
        }
        None
    }

    /// Takes the first `/find` result of the right kind.
    pub(crate) async fn find_tmdb_id_by<A: sdks::Auth + Clone>(
        external_id: String,
        external_source: &str,
        is_tv: bool,
        client: &RestClient<A>,
    ) -> anyhow::Result<Option<i64>> {
        let found = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id,
                    external_source: external_source.to_string(),
                }
                .with_cache(ID_CACHE_TTL)
                .with_cache_ttl_if(ID_MISS_CACHE_TTL, find_matched_nothing),
            )
            .await?;
        Ok(if is_tv {
            found
                .tv_results
                .into_iter()
                .next()
                .map(|s| s.id)
        } else {
            found
                .movie_results
                .into_iter()
                .next()
                .map(|m| m.id)
        })
    }

    /// The series' TMDB id, from whatever else it carries. Kitsu comes last:
    /// a series with an imdb id would pay for a mapping call it then discards.
    async fn series_tmdb_id(
        ids: &db::ExternalIds,
        client: &RestClient<BearerAuth>,
        kitsu: &RestClient,
    ) -> anyhow::Result<Option<i64>> {
        if let Some(tmdb) = ids.tmdb {
            return Ok(Some(tmdb));
        }
        let Some((external_id, external_source)) =
            Self::tmdb_search_key(ids, Some(kitsu)).await
        else {
            return Ok(None);
        };
        Self::find_tmdb_id_by(external_id, external_source, true, client).await
    }

    /// The episode's own external ids, `Ok(None)` when TMDB has no such episode
    /// or knows it by none. A 404 is cached like any other answer, so a series
    /// TMDB does not carry is not re-asked for on every delivery.
    ///
    /// Appends `external_ids` alone, not the addon's default list: the cache is
    /// keyed on the url, so a wider request would inherit the addon's TTL.
    async fn episode_external_ids(
        series_tmdb: i64,
        season: i64,
        episode: i64,
        client: &RestClient<BearerAuth>,
    ) -> anyhow::Result<Option<db::ExternalIds>> {
        let Some(ep) = client
            .execute(
                sdks::tmdb::EpisodeEndpoint {
                    series_id: series_tmdb,
                    season_number: season,
                    episode_number: episode,
                    language: None,
                    append_to_response: Some(vec!["external_ids".to_string()]),
                }
                .absent_on(404)
                .with_cache(ID_CACHE_TTL)
                .with_cache_ttl_if(ID_MISS_CACHE_TTL, Option::is_none),
            )
            .await?
        else {
            return Ok(None);
        };

        let Some(external) = ep.external_ids else {
            return Ok(None);
        };
        Ok(Some(db::ExternalIds {
            imdb: external
                .imdb_id
                .and_then(|s| db::NonEmptyString::try_new(s).ok()),
            tvdb: external.tvdb_id,
            tmdb: Some(ep.id),
            ..Default::default()
        }))
    }

    /// Fill in the ids a tracker needs to name `episode`, on it and on `series`.
    ///
    /// Providers disagree on what identifies an episode: the show's tmdb id plus
    /// season and episode, or the episode's own imdb or tvdb id. Both are closed
    /// rather than either rule encoded here.
    ///
    /// `Ok(false)` is nothing left to add; an error is worth retrying.
    pub async fn complete_episode_ids(
        episode: &mut db::Media,
        series: &mut db::Media,
        ctx: &AppContext,
    ) -> anyhow::Result<bool> {
        let series_needs_tmdb = series
            .external_ids
            .tmdb
            .is_none();
        // One response carries both of the episode's ids, so it is worth
        // asking for while either is missing.
        let episode_needs_ids = episode.kind == db::MediaKind::Episode
            && (episode
                .external_ids
                .imdb
                .is_none()
                || episode
                    .external_ids
                    .tvdb
                    .is_none());
        if !series_needs_tmdb && !episode_needs_ids {
            return Ok(false);
        }

        let Some(client) = Self::tmdb(ctx).await else {
            return Ok(false);
        };
        let mut changed =
            series_needs_tmdb && Self::fill_series_tmdb(series, ctx, &client).await?;
        if !episode_needs_ids {
            return Ok(changed);
        }

        let (Some(season), Some(number), Some(series_tmdb)) = (
            episode.parent_idx,
            episode.idx,
            series
                .external_ids
                .tmdb,
        ) else {
            return Ok(changed);
        };
        let Some(patch) =
            Self::episode_external_ids(series_tmdb, season, number, &client).await?
        else {
            return Ok(changed);
        };

        let before = episode
            .external_ids
            .clone();
        episode
            .external_ids
            .merge(&patch, false);
        if episode.external_ids != before {
            // Adopt the stored merge: it can only be wider than the local one.
            if let Some(stored) =
                db::Media::widen_external_ids(&ctx.db, &episode.id, &patch).await?
            {
                episode.external_ids = stored;
            }
            changed = true;
        }
        Ok(changed)
    }

    async fn fill_series_tmdb(
        series: &mut db::Media,
        ctx: &AppContext,
        client: &RestClient<BearerAuth>,
    ) -> anyhow::Result<bool> {
        let Some(tmdb) =
            Self::series_tmdb_id(&series.external_ids, client, &sdks::kitsu::client())
                .await?
        else {
            return Ok(false);
        };

        series
            .external_ids
            .tmdb = Some(tmdb);
        // Cannot re-key the row out from under its own episodes: `candidate_ids`
        // ranks imdb and the Stremio id above tmdb, and a series carrying
        // neither is one `Media::save` refuses.
        if let Some(stored) = db::Media::widen_external_ids(
            &ctx.db,
            &series.id,
            &db::ExternalIds {
                tmdb: Some(tmdb),
                ..Default::default()
            },
        )
        .await?
        {
            series.external_ids = stored;
        }
        Ok(true)
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
                let q = media.deezer_search_query("track");
                let hit = match client
                    .execute(dz::SearchTracksEndpoint { q, limit: 1 })
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
                let q = media.deezer_search_query("album");
                let hit = match client
                    .execute(dz::SearchAlbumsEndpoint { q, limit: 1 })
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
        // Mutated below (IMDB/Deezer resolution rewrites ids), so this one needs its
        // own copy rather than the shared handle.
        let Some(mut media) = ctx
            .store
            .get::<db::Media>(id.to_string())
            .map(|m| (*m).clone())
        else {
            return Ok(None);
        };
        ctx.store
            .delete(id.to_string());

        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
            if !Self::resolve_media_imdb(&mut media, ctx).await {
                // If the item arrived with a resolvable external ID (TMDB or TVDB), we
                // expected to derive an IMDB ID from it. Bail early so the caller sees
                // a clean failure instead of a silent crash.
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
            let raw = media.media_id_raw();
            if raw
                .canonical()
                .is_some()
            {
                media.id = uuid::Uuid::from(&raw);
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
        // process_meta_item now owns all upserts internally and returns the actual UUID
        // (which may differ from resolved_id if an existing DB row was adopted).
        let actual_id = ctx
            .addons
            .process_meta_item(root, ctx.clone(), false, config)
            .await;
        Ok(db::Media::get_by_id(&ctx.db, &actual_id).await?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use uuid::Uuid;

    fn tmdb_client(base_url: &str) -> remux_sdks::RestClient<remux_sdks::BearerAuth> {
        remux_sdks::RestClient::new(base_url)
            .unwrap()
            .with_auth(remux_sdks::BearerAuth {
                token: String::new(),
            })
    }

    fn kitsu_client(base_url: &str) -> remux_sdks::RestClient {
        remux_sdks::RestClient::new(base_url).unwrap()
    }

    async fn series_tmdb_id(
        server: &httpmock::MockServer,
        ids: db::ExternalIds,
    ) -> anyhow::Result<Option<i64>> {
        MediaResolveService::series_tmdb_id(
            &ids,
            &tmdb_client(&server.base_url()),
            &kitsu_client(&server.base_url()),
        )
        .await
    }

    fn imdb_ids(imdb: &str) -> db::ExternalIds {
        db::ExternalIds {
            imdb: db::NonEmptyString::try_new(imdb.to_string()).ok(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_series_known_only_by_imdb_still_finds_its_tmdb_id() {
        let server = httpmock::MockServer::start();
        let find = server.mock(|when, then| {
            when.path("/find/tt0306414")
                .query_param("external_source", "imdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 1438, "name": "The Wire"}],
                    "movie_results": []
                }));
        });

        assert_eq!(
            series_tmdb_id(&server, imdb_ids("tt0306414"))
                .await
                .unwrap(),
            Some(1438)
        );
        find.assert();
    }

    #[tokio::test]
    async fn a_series_that_already_knows_its_tmdb_id_asks_nobody() {
        let server = httpmock::MockServer::start();
        let find = server.mock(|when, then| {
            when.path_contains("/find/");
            then.status(200)
                .json_body(serde_json::json!({"tv_results": [], "movie_results": []}));
        });

        let ids = db::ExternalIds {
            tmdb: Some(1438),
            ..imdb_ids("tt0306414")
        };
        assert_eq!(
            series_tmdb_id(&server, ids)
                .await
                .unwrap(),
            Some(1438)
        );
        assert_eq!(find.hits(), 0);
    }

    #[tokio::test]
    async fn a_series_with_no_id_tmdb_reads_resolves_to_nothing() {
        let server = httpmock::MockServer::start();
        let ids = db::ExternalIds {
            deezer_artist: Some(7),
            ..Default::default()
        };
        assert!(
            series_tmdb_id(&server, ids)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Anime is often on kitsu and nowhere else.
    #[tokio::test]
    async fn an_anime_series_is_found_through_kitsu() {
        let server = httpmock::MockServer::start();
        let mappings = server.mock(|when, then| {
            when.path("/anime/42/mappings");
            then.status(200)
                .json_body(serde_json::json!({
                    "data": [{
                        "attributes": {
                            "externalSite": "thetvdb",
                            "externalId": "4165880"
                        }
                    }]
                }));
        });
        let find = server.mock(|when, then| {
            when.path("/find/4165880")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 157842, "name": "Black Summoner"}],
                    "movie_results": []
                }));
        });

        let ids = db::ExternalIds {
            kitsu: Some(42),
            ..Default::default()
        };
        assert_eq!(
            series_tmdb_id(&server, ids)
                .await
                .unwrap(),
            Some(157842)
        );
        mappings.assert();
        find.assert();
    }

    /// `None` would spend the delivery on an outage.
    #[tokio::test]
    async fn tmdb_being_unreachable_is_an_error_rather_than_no_such_series() {
        // Ids of its own throughout these tests: the response cache is keyed
        // on the url and httpmock reuses servers.
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/tt0000503");
            then.status(503);
        });

        assert!(
            series_tmdb_id(&server, imdb_ids("tt0000503"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_episode_carries_ids_the_season_response_does_not() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/tv/1438/season/1/episode/1");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": 972467,
                    "name": "The Target",
                    "episode_number": 1,
                    "season_number": 1,
                    "external_ids": {
                        "imdb_id": "tt0749419",
                        "tvdb_id": 303821,
                    },
                }));
        });

        let ids = MediaResolveService::episode_external_ids(
            1438,
            1,
            1,
            &tmdb_client(&server.base_url()),
        )
        .await
        .unwrap()
        .expect("the episode exists");

        assert_eq!(
            ids.imdb
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt0749419"),
            "Yamtrack matches an episode on this"
        );
        assert_eq!(ids.tvdb, Some(303821));
        assert_eq!(ids.tmdb, Some(972467));
    }

    #[tokio::test]
    async fn an_episode_tmdb_does_not_know_is_not_an_error() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/tv/1438/season/9/episode/9");
            then.status(404)
                .json_body(serde_json::json!({ "status_code": 34 }));
        });

        assert!(
            MediaResolveService::episode_external_ids(
                1438,
                9,
                9,
                &tmdb_client(&server.base_url()),
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    /// Every delivery for the same episode would otherwise re-ask, since the
    /// client caches nothing it did not get a 2xx for.
    #[tokio::test]
    async fn an_episode_tmdb_does_not_know_is_asked_for_once() {
        let server = httpmock::MockServer::start();
        let missing = server.mock(|when, then| {
            when.path("/tv/5150/season/7/episode/7");
            then.status(404)
                .json_body(serde_json::json!({ "status_code": 34 }));
        });
        let client = tmdb_client(&server.base_url());

        for _ in 0..2 {
            assert!(
                MediaResolveService::episode_external_ids(5150, 7, 7, &client)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        assert_eq!(missing.hits(), 1, "the miss was not cached");
    }

    /// The UUID this row would be derived as, were it ingested again.
    fn derived_id(media: &db::Media) -> Uuid {
        Uuid::from(&db::MediaIdRaw {
            kind: media
                .kind
                .clone(),
            external_ids: media
                .external_ids
                .clone(),
            season: None,
            episode: None,
        })
    }

    /// A series carrying nothing but `ids`, saved and handed back.
    async fn series(ctx: &crate::AppContext, ids: db::ExternalIds) -> db::Media {
        let mut media = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: ids.clone(),
                season: None,
                episode: None,
            }),
            title: "The Wire".into(),
            kind: db::MediaKind::Series,
            external_ids: ids,
            ..Default::default()
        };
        media
            .save(&ctx.db)
            .await
            .unwrap();
        media
    }

    /// The id the row is keyed on outranks tmdb in `candidate_ids`, so storing
    /// one leaves the UUID a later ingest derives untouched.
    #[tokio::test]
    async fn storing_a_series_tmdb_id_does_not_re_key_the_row() {
        let tmdb = httpmock::MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/find/tt7770001");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{ "id": 7777, "name": "Show" }],
                    "movie_results": []
                }));
        });
        let (_s, guard) =
            crate::integration_test::new_test_server_with_config(crate::Config {
                database_url: Some("sqlite::memory:".into()),
                torrent_http_port: None,
                disable_dht: true,
                tmdb_base_url: tmdb.base_url(),
                ..Default::default()
            })
            .await
            .unwrap();
        let ctx = &guard.0;
        let mut media = series(
            ctx,
            db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt7770001".to_string()).ok(),
                ..Default::default()
            },
        )
        .await;
        let keyed_on = derived_id(&media);

        let client = MediaResolveService::tmdb(ctx)
            .await
            .unwrap();
        assert!(
            MediaResolveService::fill_series_tmdb(&mut media, ctx, &client)
                .await
                .unwrap()
        );

        assert_eq!(
            db::Media::get_by_id(&ctx.db, &media.id)
                .await
                .unwrap()
                .expect("still there")
                .external_ids
                .tmdb,
            Some(7777)
        );
        assert_eq!(derived_id(&media), keyed_on);
    }

    /// A server whose TMDB calls go to `mock`.
    async fn ctx_with_tmdb(
        mock: &httpmock::MockServer,
    ) -> crate::integration_test::TestGuard {
        crate::integration_test::new_test_server_with_config(crate::Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            disable_dht: true,
            tmdb_base_url: mock.base_url(),
            ..Default::default()
        })
        .await
        .unwrap()
        .1
    }

    /// A series tmdb could re-key is one `save` refuses outright, which is why
    /// the completion path needs no re-key guard of its own.
    #[tokio::test]
    async fn a_series_tmdb_could_re_key_cannot_be_stored_at_all() {
        let (_s, guard) =
            crate::integration_test::new_test_server_with_config(crate::Config {
                database_url: Some("sqlite::memory:".into()),
                torrent_http_port: None,
                disable_dht: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let ids = db::ExternalIds {
            tvdb: Some(7770002),
            kitsu: Some(7770012),
            ..Default::default()
        };
        let mut media = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: ids.clone(),
                season: None,
                episode: None,
            }),
            title: "Show".into(),
            kind: db::MediaKind::Series,
            external_ids: ids,
            ..Default::default()
        };
        assert!(
            media
                .save(
                    &guard
                        .0
                        .db
                )
                .await
                .is_err()
        );
    }

    /// One provider needs only the episode's own ids, another the show's tmdb
    /// id, so completion runs for the series half regardless.
    #[tokio::test]
    async fn a_series_gets_its_tmdb_id_even_when_the_episode_needs_nothing() {
        let tmdb = httpmock::MockServer::start();
        let find = tmdb.mock(|when, then| {
            when.path("/find/tt7770003");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{ "id": 7779, "name": "Show" }],
                    "movie_results": []
                }));
        });
        let episode_lookup = tmdb.mock(|when, then| {
            when.path_contains("/season/");
            then.status(200)
                .json_body(serde_json::json!({ "id": 1, "external_ids": {} }));
        });
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;
        let mut show = series(
            ctx,
            db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt7770003".to_string()).ok(),
                ..Default::default()
            },
        )
        .await;
        let mut episode = db::Media {
            id: Uuid::new_v4(),
            title: "Ep".into(),
            kind: db::MediaKind::Episode,
            parent_idx: Some(1),
            idx: Some(1),
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt7770013".to_string()).ok(),
                tvdb: Some(7770023),
                ..Default::default()
            },
            ..Default::default()
        };

        MediaResolveService::complete_episode_ids(&mut episode, &mut show, ctx)
            .await
            .unwrap();

        assert_eq!(
            show.external_ids
                .tmdb,
            Some(7779)
        );
        find.assert();
        assert_eq!(
            episode_lookup.hits(),
            0,
            "the episode already carries both ids that call would return"
        );
    }

    fn track(
        grandparent_title: Option<&str>,
        artist_name: Option<&str>,
        description: Option<&str>,
        title: &str,
    ) -> db::Media {
        db::Media {
            title: title.to_string(),
            description: description.map(String::from),
            grandparent: grandparent_title.map(|t| db::Media::stub(Uuid::new_v4(), t)),
            external_ids: db::ExternalIds {
                artist_name: artist_name.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn artist_from_grandparent_stub_wins() {
        let media = track(Some("Adele"), Some("Wrong"), None, "Hello");
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_from_flat_name_for_playlist_imports() {
        // Playlist import: no grandparent stub, flat artist_name is the source.
        let media = track(None, Some("Adele"), None, "Hello");
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_from_description_prefix() {
        let media = track(None, None, Some("by Adele"), "Hello");
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn empty_names_are_ignored() {
        let media = track(None, Some(""), Some("by "), "Hello");
        assert_eq!(media.artist_name(), None);
    }

    #[test]
    fn deezer_query_pins_artist_when_known() {
        let media = track(None, Some("Adele"), None, "Hello");
        assert_eq!(
            media.deezer_search_query("track"),
            "artist:\"Adele\" track:\"Hello\""
        );
    }

    #[test]
    fn deezer_query_strips_quotes_from_values() {
        let media = db::Media {
            title: "So \"Special\"".to_string(),
            external_ids: db::ExternalIds {
                artist_name: Some("A\"B".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            media.deezer_search_query("album"),
            "artist:\"AB\" album:\"So Special\""
        );
    }

    #[test]
    fn deezer_query_title_only_without_artist() {
        let media = track(None, None, None, "Hello");
        assert_eq!(media.deezer_search_query("track"), "Hello");
    }

    fn mock_tv_series(server: &httpmock::MockServer, tmdb_id: i64, imdb_id: &str) {
        let imdb = imdb_id.to_string();
        server.mock(|when, then| {
            when.path(format!("/tv/{tmdb_id}"));
            then.status(200)
                .json_body(serde_json::json!({
                    "id": tmdb_id,
                    "external_ids": { "imdb_id": imdb }
                }));
        });
    }

    /// Not via `resolve_imdb_from_ids`, which reaches for the real kitsu client.
    #[tokio::test]
    async fn an_anime_series_is_searched_for_by_its_kitsu_mapping() {
        let server = httpmock::MockServer::start();
        let mappings = server.mock(|when, then| {
            when.path("/anime/42/mappings");
            then.status(200)
                .json_body(serde_json::json!({
                    "data": [{
                        "attributes": {
                            "externalSite": "thetvdb",
                            "externalId": "4165880"
                        }
                    }]
                }));
        });

        assert_eq!(
            MediaResolveService::tmdb_search_key(
                &db::ExternalIds {
                    kitsu: Some(42),
                    ..Default::default()
                },
                Some(&kitsu_client(&server.base_url())),
            )
            .await,
            Some(("4165880".to_string(), "tvdb_id"))
        );
        mappings.assert();
    }

    #[tokio::test]
    async fn a_direct_imdb_id_is_searched_for_without_asking_kitsu() {
        let server = httpmock::MockServer::start();
        let mappings = server.mock(|when, then| {
            when.path("/anime/43/mappings");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        assert_eq!(
            MediaResolveService::tmdb_search_key(
                &db::ExternalIds {
                    imdb: db::NonEmptyString::try_new("tt0306414".to_string()).ok(),
                    kitsu: Some(43),
                    ..Default::default()
                },
                Some(&kitsu_client(&server.base_url())),
            )
            .await,
            Some(("tt0306414".to_string(), "imdb_id"))
        );
        assert_eq!(mappings.hits(), 0, "kitsu was never needed");
    }

    #[test]
    fn a_find_that_matched_nothing_is_a_miss() {
        assert!(find_matched_nothing(
            &sdks::tmdb::FindByIdResponse::default()
        ));
    }

    #[test]
    fn a_find_that_matched_the_other_kind_is_not_a_miss() {
        assert!(!find_matched_nothing(&sdks::tmdb::FindByIdResponse {
            movie_results: vec![sdks::tmdb::Movie::default()],
            tv_results: vec![],
        }));
    }

    /// Three call sites used to walk to the series themselves and disagreed
    /// on how. These pin what the one they share now does.
    mod stored_series_tmdb_id {
        use super::*;
        use crate::integration_test::new_test_server;

        /// A series' id has to be the one its external ids hash to, which
        /// `Media::save` enforces.
        async fn seed(
            ctx: &AppContext,
            series_tmdb: Option<i64>,
            season_tmdb: Option<i64>,
        ) -> (db::Media, db::Media) {
            let external_ids = db::ExternalIds {
                tmdb: series_tmdb,
                imdb: db::NonEmptyString::try_new("tt0306414".to_string()).ok(),
                ..Default::default()
            };
            let mut series = db::Media {
                id: uuid::Uuid::from(&db::MediaIdRaw {
                    kind: db::MediaKind::Series,
                    external_ids: external_ids.clone(),
                    season: None,
                    episode: None,
                }),
                title: "The Wire".into(),
                kind: db::MediaKind::Series,
                external_ids,
                ..Default::default()
            };
            series
                .save(&ctx.db)
                .await
                .unwrap();

            let mut season = db::Media {
                title: "Season 1".into(),
                kind: db::MediaKind::Season,
                parent_id: Some(series.id),
                grandparent_id: Some(series.id),
                idx: Some(1),
                external_ids: db::ExternalIds {
                    tmdb: season_tmdb,
                    ..Default::default()
                },
                ..Default::default()
            };
            season
                .save(&ctx.db)
                .await
                .unwrap();
            (series, season)
        }

        #[tokio::test]
        async fn an_episode_walks_up_its_grandparent_id() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let (series, season) = seed(ctx, Some(1438), None).await;
            let episode = db::Media {
                kind: db::MediaKind::Episode,
                parent_id: Some(season.id),
                grandparent_id: Some(series.id),
                ..Default::default()
            };

            assert_eq!(
                MediaResolveService::stored_series_tmdb_id(&episode, ctx)
                    .await
                    .unwrap(),
                Some(1438)
            );
        }

        /// The filter is all that stands between a corrupt `grandparent_id`
        /// and a season's own tmdb id being spent as the series'.
        #[tokio::test]
        async fn a_grandparent_that_is_not_a_series_is_ignored() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let (_series, season) = seed(ctx, Some(1438), Some(9999)).await;
            let episode = db::Media {
                kind: db::MediaKind::Episode,
                grandparent_id: Some(season.id),
                ..Default::default()
            };

            assert_eq!(
                MediaResolveService::stored_series_tmdb_id(&episode, ctx)
                    .await
                    .unwrap(),
                None
            );
        }

        /// A season still has to reach its series past the kind filter.
        #[tokio::test]
        async fn a_season_reaches_its_series() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let (_series, season) = seed(ctx, Some(1438), None).await;

            assert_eq!(
                MediaResolveService::stored_series_tmdb_id(&season, ctx)
                    .await
                    .unwrap(),
                Some(1438)
            );
        }

        /// A caller that ran `preload_parents` pays for no query here.
        #[tokio::test]
        async fn a_preloaded_grandparent_wins() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let (series, season) = seed(ctx, Some(1438), None).await;
            let episode = db::Media {
                kind: db::MediaKind::Episode,
                parent_id: Some(season.id),
                grandparent_id: Some(series.id),
                grandparent: Some(Box::new(db::Media {
                    kind: db::MediaKind::Series,
                    external_ids: db::ExternalIds {
                        tmdb: Some(4242),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
                ..Default::default()
            };

            assert_eq!(
                MediaResolveService::stored_series_tmdb_id(&episode, ctx)
                    .await
                    .unwrap(),
                Some(4242)
            );
        }

        /// A corrupt `grandparent_id` reaches the preloaded branch too.
        #[tokio::test]
        async fn a_preloaded_grandparent_that_is_not_a_series_is_ignored() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let (_series, season) = seed(ctx, Some(1438), Some(9999)).await;
            let episode = db::Media {
                kind: db::MediaKind::Episode,
                grandparent_id: Some(season.id),
                grandparent: Some(Box::new(db::Media {
                    kind: db::MediaKind::Season,
                    external_ids: db::ExternalIds {
                        tmdb: Some(4242),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
                ..Default::default()
            };

            assert_eq!(
                MediaResolveService::stored_series_tmdb_id(&episode, ctx)
                    .await
                    .unwrap(),
                None
            );
        }
    }

    /// The addon caches the same two paths for six minutes, and the response
    /// cache is keyed on the url, so an id lookup must not ask for its url.
    #[test]
    fn id_lookups_do_not_share_the_addon_s_metadata_url() {
        use remux_sdks::Endpoint;

        for (ids, meta) in [
            (
                series_ids_endpoint(1438).query(),
                sdks::tmdb::SeriesEndpoint::new(1438, None).query(),
            ),
            (
                movie_ids_endpoint(949).query(),
                sdks::tmdb::MovieEndpoint::new(949, None).query(),
            ),
        ] {
            assert_ne!(ids, meta);
            assert!(
                ids.contains(&("append_to_response".into(), "external_ids".into()))
            );
        }
    }

    /// `deezer_artist` is one such id.
    #[tokio::test]
    async fn ids_tmdb_cannot_search_on_produce_no_key() {
        assert!(
            MediaResolveService::tmdb_search_key(
                &db::ExternalIds {
                    deezer_artist: Some(7),
                    ..Default::default()
                },
                None,
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_black_summoner() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/416588")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 157842, "name": "Black Summoner"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 157842, "tt21249100");

        let ids = db::ExternalIds {
            tvdb: Some(416588),
            ..Default::default()
        };
        let result = MediaResolveService::resolve_imdb_from_ids(
            &ids,
            true,
            &tmdb_client(&server.base_url()),
        )
        .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt21249100"),
            "Black Summoner tvdbid-416588"
        );
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_bleach() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/74796")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 30984, "name": "Bleach"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 30984, "tt0434665");

        let ids = db::ExternalIds {
            tvdb: Some(74796),
            ..Default::default()
        };
        let result = MediaResolveService::resolve_imdb_from_ids(
            &ids,
            true,
            &tmdb_client(&server.base_url()),
        )
        .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt0434665"),
            "Bleach tvdbid-74796"
        );
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_blood_c() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/249864")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 43270, "name": "Blood-C"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 43270, "tt1890725");

        let ids = db::ExternalIds {
            tvdb: Some(249864),
            ..Default::default()
        };
        let result = MediaResolveService::resolve_imdb_from_ids(
            &ids,
            true,
            &tmdb_client(&server.base_url()),
        )
        .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt1890725"),
            "Blood-C tvdbid-249864"
        );
    }
}
