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
    sdks::{CachedEndpoint, ClientError},
};

/// An id mapping between TMDB and another provider is effectively permanent,
/// unlike the metadata endpoints the addon caches for minutes.
const ID_CACHE_TTL: Duration = Duration::from_secs(86400);

pub struct MediaResolveService;

impl MediaResolveService {
    /// `None` when no TMDB key is configured. Reads the settings row, so a
    /// path needing more than one lookup builds it once and passes it down.
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

    /// The series' TMDB id, from whatever else it carries.
    ///
    /// Kitsu is the last fallback rather than the first: anime is often on
    /// kitsu and nowhere else, but a series with an imdb id would pay for a
    /// mapping call whose result it then discards.
    async fn series_tmdb_id(
        ids: &db::ExternalIds,
        client: &RestClient<BearerAuth>,
        kitsu: &RestClient,
    ) -> anyhow::Result<Option<i64>> {
        if let Some(tmdb) = ids.tmdb {
            return Ok(Some(tmdb));
        }

        let (external_id, external_source): (String, &str) =
            if let Some(imdb) = &ids.imdb {
                (
                    imdb.clone()
                        .into(),
                    "imdb_id",
                )
            } else if let Some(tvdb) = ids.tvdb {
                (tvdb.to_string(), "tvdb_id")
            } else if let Some(kitsu_id) = ids.kitsu {
                match kitsu
                    .execute(sdks::kitsu::MappingsEndpoint { kitsu_id })
                    .await
                {
                    Ok(m) => match m.tvdb_id() {
                        Some(tvdb) => (tvdb.to_string(), "tvdb_id"),
                        None => return Ok(None),
                    },
                    Err(e) => {
                        warn!(kitsu_id, error = %e, "kitsu mappings lookup failed");
                        return Ok(None);
                    }
                }
            } else {
                return Ok(None);
            };

        let found = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id,
                    external_source: external_source.to_string(),
                }
                .with_cache(ID_CACHE_TTL),
            )
            .await?;
        Ok(found
            .tv_results
            .into_iter()
            .next()
            .map(|s| s.id))
    }

    /// The episode's own external ids. `Ok(None)` when TMDB has no such
    /// episode, or knows it by no other id.
    ///
    /// Appends `external_ids` alone rather than the addon's default list: the
    /// response cache is keyed on the url, so a wider request would share an
    /// entry with the addon's episode call and inherit its TTL.
    async fn episode_external_ids(
        series_tmdb: i64,
        season: i64,
        episode: i64,
        client: &RestClient<BearerAuth>,
    ) -> anyhow::Result<Option<db::ExternalIds>> {
        let ep = match client
            .execute(
                sdks::tmdb::EpisodeEndpoint {
                    series_id: series_tmdb,
                    season_number: season,
                    episode_number: episode,
                    language: None,
                    append_to_response: Some(vec!["external_ids".to_string()]),
                }
                .with_cache(ID_CACHE_TTL),
            )
            .await
        {
            Ok(ep) => ep,
            Err(
                ClientError::Http { status: 404, .. }
                | ClientError::Json { status: 404, .. },
            ) => return Ok(None),
            Err(e) => return Err(e.into()),
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

    /// Fill in the ids a media tracker needs to name `episode`, on the episode
    /// and on the `series` it hangs off.
    ///
    /// Providers disagree about which identifies an episode: one keys on the
    /// show's tmdb id plus season and episode, another on the episode's own
    /// imdb or tvdb id. Closing both beats encoding either rule here.
    ///
    /// `Ok(false)` is nothing left to add; an error is a lookup worth retrying.
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
            db::Media::update_external_ids(&ctx.db, &episode.id, &episode.external_ids)
                .await?;
            changed = true;
        }
        Ok(changed)
    }

    /// Resolve the series' TMDB id and store it on the row.
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
        db::Media::update_external_ids(&ctx.db, &series.id, &series.external_ids)
            .await?;
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
    use super::MediaResolveService;
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

    /// The case the kitsu fallback exists for: anime is often on kitsu and
    /// nowhere else.
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

    /// What lets the completion path store a tmdb id without a re-key guard of
    /// its own: a series tmdb could re-key is one `save` refuses outright.
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
}
