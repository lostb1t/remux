use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use futures::{Stream, stream};
use remux_sdks::simkl::{
    DeviceTokenPoll, DeviceTokenResponse, SimklAnimeType, SimklClient,
    SimklCredentials, SimklCustomList, SimklCustomListItem, SimklCustomListMediaType,
    SimklEpisodeDetail, SimklError, SimklItemIds, SimklLibrary, SimklLibraryEntry,
    SimklMedia, SimklMediaType, SimklStatus,
};
use remux_utils::Secret;
use serde_json::Value;
use std::pin::Pin;
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    CatalogAddon, CatalogInfo, MediaKind, MetaAddon, ResourceType, TreeAddon,
    media_tracker::{
        AuthFlow, DeviceAuthPoll, DeviceAuthStart, MediaTrackerAddon,
        MediaTrackerCapabilities, MediaTrackerConnection, MediaTrackerCredentials,
        MediaTrackerCtx, MediaTrackerError, MediaTrackerEvent, MediaTrackerResult,
        MediaTrackerTarget,
    },
};
use crate::{AppContext, api, db};

const SIMKL_ID_PREFIX: &str = "simkl:";
const SIMKL_TV_TYPE: &str = "simkl-tv";
const SIMKL_ANIME_TYPE: &str = "simkl-anime";
const SIMKL_MOVIE_TYPE: &str = "simkl-movie";
const COMPLETE_CATALOG_LIMIT: i64 = 999_999_999;
const SIMKL_STATUSES: [SimklStatus; 5] = [
    SimklStatus::Watching,
    SimklStatus::Plantowatch,
    SimklStatus::Hold,
    SimklStatus::Completed,
    SimklStatus::Dropped,
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum SimklCatalogId {
    History,
    Status {
        account_id: String,
        status: SimklStatus,
    },
    CustomList {
        account_id: String,
        list_id: i64,
    },
}

impl std::fmt::Display for SimklCatalogId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History => formatter.write_str("history"),
            Self::Status { account_id, status } => {
                write!(formatter, "account:{account_id}:status:{status}")
            }
            Self::CustomList {
                account_id,
                list_id,
            } => write!(formatter, "account:{account_id}:list:{list_id}"),
        }
    }
}

impl FromStr for SimklCatalogId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "history" {
            return Ok(Self::History);
        }
        let parts = value
            .split(':')
            .collect::<Vec<_>>();
        match parts.as_slice() {
            ["account", account_id, "status", status] => Ok(Self::Status {
                account_id: (*account_id).to_string(),
                status: SimklStatus::from_str(status).map_err(|_| {
                    anyhow::anyhow!("unknown Simkl status catalog: {status}")
                })?,
            }),
            ["account", account_id, "list", list_id] => Ok(Self::CustomList {
                account_id: (*account_id).to_string(),
                list_id: list_id.parse()?,
            }),
            _ => anyhow::bail!("unknown Simkl catalog: {value}"),
        }
    }
}

pub struct SimklPreset;

impl AddonPreset for SimklPreset {
    fn id(&self) -> &'static str {
        "simkl"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "simkl".into(),
            display_name: "Simkl".into(),
            description:
                "Imports Simkl history, watchlist statuses, and custom lists as Remux catalogs for Crosswatch."
                    .into(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Catalog),
                AddonMetadata::simple_resource(ResourceType::Meta),
            ],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Series,
                MediaKind::Episode,
            ],
            supported_resources_user: vec![],
            supported_types_user: vec![],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        _cfg: &Value,
        config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let client = config
            .simkl_client_id
            .as_ref()
            .filter(|client_id| {
                !client_id
                    .trim()
                    .is_empty()
            })
            .map(|client_id| SimklClient::new(&config.simkl_api_base_url, client_id));
        let addon = Arc::new(SimklAddon {
            addon_id,
            client,
            episode_cache: Mutex::new(HashMap::new()),
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            media_tracker: Some(addon.clone()),
            meta: Some(addon.clone()),
            tree: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(SimklPreset))
}

pub struct SimklAddon {
    addon_id: Uuid,
    client: Option<SimklClient>,
    episode_cache: Mutex<HashMap<String, Arc<Vec<SimklEpisodeDetail>>>>,
}

impl SimklAddon {
    fn client(&self) -> MediaTrackerResult<&SimklClient> {
        self.client
            .as_ref()
            .ok_or_else(|| {
                MediaTrackerError::permanent(
                    "Simkl is not configured; set simkl_client_id",
                )
            })
    }

    fn parse_credentials(
        credentials: &MediaTrackerCredentials,
    ) -> MediaTrackerResult<SimklCredentials> {
        serde_json::from_value(
            credentials
                .expose()
                .clone(),
        )
        .map_err(|_| MediaTrackerError::reauth("stored Simkl credentials are invalid"))
    }

    fn wrap_credentials(
        credentials: &SimklCredentials,
    ) -> MediaTrackerResult<MediaTrackerCredentials> {
        serde_json::to_value(credentials)
            .map(Secret::new)
            .map_err(|error| {
                MediaTrackerError::permanent(format!(
                    "could not store Simkl credentials: {error}"
                ))
            })
    }

    fn map_error(error: SimklError) -> MediaTrackerError {
        match error {
            SimklError::Unauthorized => {
                MediaTrackerError::reauth("Simkl authorization is invalid or expired")
            }
            SimklError::RateLimited {
                retry_after: Some(after),
            } => MediaTrackerError::retry_after("Simkl rate limit reached", after),
            other if other.is_retryable() => {
                MediaTrackerError::retryable(other.to_string())
            }
            other => MediaTrackerError::permanent(other.to_string()),
        }
    }

    fn parse_numeric(value: Option<&String>) -> Option<i64> {
        value.and_then(|value| {
            value
                .parse()
                .ok()
        })
    }

    fn ids(ids: &SimklItemIds, media_type: &str) -> db::ExternalIds {
        db::ExternalIds {
            imdb: ids
                .imdb
                .as_ref()
                .and_then(|id| db::NonEmptyString::try_new(id.clone()).ok()),
            tmdb: Self::parse_numeric(
                ids.tmdb
                    .as_ref(),
            ),
            tvdb: Self::parse_numeric(
                ids.tvdb
                    .as_ref(),
            ),
            kitsu: Self::parse_numeric(
                ids.kitsu
                    .as_ref(),
            ),
            custom_stremio_id: ids
                .simkl_id()
                .map(|id| format!("{SIMKL_ID_PREFIX}{id}")),
            custom_stremio_type: Some(media_type.to_string()),
            ..Default::default()
        }
    }

    fn catalog_media(
        media: &SimklMedia,
        media_type: &str,
        kind: db::MediaKind,
    ) -> db::Media {
        let external_ids = Self::ids(&media.ids, media_type);
        let raw = db::MediaIdRaw {
            kind: kind.clone(),
            external_ids: external_ids.clone(),
            season: None,
            episode: None,
        };
        db::Media {
            id: Uuid::from(&raw),
            title: media
                .title
                .clone(),
            kind,
            released_at: media
                .year
                .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, 1, 1))
                .and_then(|date| date.and_hms_opt(0, 0, 0)),
            external_ids,
            ..Default::default()
        }
    }

    fn custom_list_media(item: &SimklCustomListItem) -> db::Media {
        let (media_type, kind) = match item.kind {
            SimklMediaType::Movie => (SIMKL_MOVIE_TYPE, db::MediaKind::Movie),
            SimklMediaType::Tv => (SIMKL_TV_TYPE, db::MediaKind::Series),
            SimklMediaType::Anime if item.anime_type == Some(SimklAnimeType::Movie) => {
                (SIMKL_MOVIE_TYPE, db::MediaKind::Movie)
            }
            SimklMediaType::Anime => (SIMKL_ANIME_TYPE, db::MediaKind::Series),
        };
        let external_ids = Self::ids(&item.ids, media_type);
        let raw = db::MediaIdRaw {
            kind: kind.clone(),
            external_ids: external_ids.clone(),
            season: None,
            episode: None,
        };
        db::Media {
            id: Uuid::from(&raw),
            title: item
                .title
                .clone(),
            kind,
            description: item
                .overview
                .clone(),
            released_at: item
                .release_date
                .map(|date| date.naive_utc())
                .or_else(|| {
                    item.year
                        .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, 1, 1))
                        .and_then(|date| date.and_hms_opt(0, 0, 0))
                }),
            external_ids,
            ..Default::default()
        }
    }

    fn deduplicate_roots(roots: &mut Vec<db::Media>) {
        let mut seen = HashSet::new();
        roots.retain(|media| {
            media
                .media_id_raw()
                .identity_key()
                .map(|key| seen.insert(key))
                .unwrap_or_else(|| {
                    seen.insert(
                        media
                            .id
                            .to_string(),
                    )
                })
        });
    }

    fn library_roots(library: SimklLibrary, history_only: bool) -> Vec<db::Media> {
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |media: db::Media| {
            let key = media
                .media_id_raw()
                .identity_key()
                .unwrap_or_else(|| {
                    media
                        .id
                        .to_string()
                });
            if seen.insert(key) {
                roots.push(media);
            }
        };

        for entry in library.movies {
            if (!history_only || Self::entry_has_history(&entry))
                && let Some(movie) = entry
                    .movie
                    .as_ref()
            {
                push(Self::catalog_media(
                    &movie,
                    SIMKL_MOVIE_TYPE,
                    db::MediaKind::Movie,
                ));
            }
        }
        for entry in library.shows {
            if (!history_only || Self::entry_has_history(&entry))
                && let Some(show) = entry
                    .show
                    .as_ref()
            {
                push(Self::catalog_media(
                    &show,
                    SIMKL_TV_TYPE,
                    db::MediaKind::Series,
                ));
            }
        }
        for entry in library.anime {
            if (!history_only || Self::entry_has_history(&entry))
                && let Some(anime) = entry
                    .show
                    .as_ref()
            {
                let (media_type, kind) =
                    if entry.anime_type == Some(SimklAnimeType::Movie) {
                        (SIMKL_MOVIE_TYPE, db::MediaKind::Movie)
                    } else {
                        (SIMKL_ANIME_TYPE, db::MediaKind::Series)
                    };
                push(Self::catalog_media(&anime, media_type, kind));
            }
        }
        roots
    }

    fn custom_list_roots(items: Vec<SimklCustomListItem>) -> Vec<db::Media> {
        let mut roots = items
            .iter()
            .map(Self::custom_list_media)
            .collect::<Vec<_>>();
        Self::deduplicate_roots(&mut roots);
        roots
    }

    fn tracker_account_id(tracker: &db::UserMediaTracker) -> String {
        tracker
            .remote_account_id
            .clone()
            .unwrap_or_else(|| {
                tracker
                    .id
                    .to_string()
            })
    }

    fn tracker_account_name(tracker: &db::UserMediaTracker) -> String {
        tracker
            .remote_account_name
            .clone()
            .or_else(|| {
                tracker
                    .remote_account_id
                    .clone()
            })
            .unwrap_or_else(|| {
                tracker
                    .user_id
                    .to_string()
            })
    }

    fn status_name(status: SimklStatus) -> &'static str {
        match status {
            SimklStatus::Watching => "Watching",
            SimklStatus::Plantowatch => "Plan to Watch",
            SimklStatus::Hold => "On Hold",
            SimklStatus::Completed => "Completed",
            SimklStatus::Dropped => "Dropped",
        }
    }

    fn catalog_info(
        id: SimklCatalogId,
        name: impl Into<String>,
        collection_media_kind: db::CollectionMediaKind,
        media_kind: Option<db::MediaKind>,
        default_enabled: bool,
    ) -> CatalogInfo {
        CatalogInfo {
            default_enabled,
            default_max_items: Some(COMPLETE_CATALOG_LIMIT),
            collection_media_kind: Some(collection_media_kind),
            media_kind,
            ..CatalogInfo::new(id.to_string(), name)
        }
    }

    fn custom_list_catalog(
        account_id: &str,
        account_name: &str,
        list: &SimklCustomList,
    ) -> Option<CatalogInfo> {
        let (collection_kind, media_kind) = match list.media_type {
            SimklCustomListMediaType::Movies => {
                (db::CollectionMediaKind::Movie, Some(db::MediaKind::Movie))
            }
            SimklCustomListMediaType::Tv => {
                (db::CollectionMediaKind::Series, Some(db::MediaKind::Series))
            }
            SimklCustomListMediaType::Anime => (db::CollectionMediaKind::Mixed, None),
            _ => return None,
        };
        Some(Self::catalog_info(
            SimklCatalogId::CustomList {
                account_id: account_id.to_string(),
                list_id: list.id,
            },
            format!("Simkl - {account_name} - {}", list.name),
            collection_kind,
            media_kind,
            false,
        ))
    }

    fn entry_has_history(entry: &SimklLibraryEntry) -> bool {
        entry.status == SimklStatus::Completed
            || entry
                .last_watched_at
                .is_some()
            || entry
                .seasons
                .iter()
                .flat_map(|season| &season.episodes)
                .any(|episode| {
                    episode
                        .watched_at
                        .is_some()
                })
    }

    async fn fetch_library(
        &self,
        ctx: &AppContext,
        tracker: &mut db::UserMediaTracker,
    ) -> Result<SimklLibrary> {
        let mut credentials = Self::parse_credentials(&tracker.credentials)?;
        let mut result = self
            .client()?
            .library(&credentials)
            .await;
        if matches!(&result, Err(SimklError::Unauthorized)) {
            let refreshed = self
                .client()?
                .refresh(
                    credentials
                        .refresh_token
                        .expose(),
                )
                .await
                .map_err(Self::map_error)?;
            let wrapped = Self::wrap_credentials(&refreshed)?;
            db::UserMediaTracker::replace_credentials(&ctx.db, tracker.id, &wrapped)
                .await?;
            tracker.credentials = wrapped;
            credentials = refreshed;
            result = self
                .client()?
                .library(&credentials)
                .await;
        }
        result
            .map_err(Self::map_error)
            .map_err(Into::into)
    }

    async fn fetch_status_library(
        &self,
        ctx: &AppContext,
        tracker: &mut db::UserMediaTracker,
        status: SimklStatus,
    ) -> Result<SimklLibrary> {
        let mut credentials = Self::parse_credentials(&tracker.credentials)?;
        let mut result = self
            .client()?
            .library_by_status(&credentials, status)
            .await;
        if matches!(&result, Err(SimklError::Unauthorized)) {
            credentials = self
                .refresh_and_store_credentials(ctx, tracker, &credentials)
                .await?;
            result = self
                .client()?
                .library_by_status(&credentials, status)
                .await;
        }
        result
            .map_err(Self::map_error)
            .map_err(Into::into)
    }

    async fn fetch_custom_lists(
        &self,
        ctx: &AppContext,
        tracker: &mut db::UserMediaTracker,
        user_id: i64,
    ) -> Result<Vec<SimklCustomList>> {
        let mut credentials = Self::parse_credentials(&tracker.credentials)?;
        let mut result = self
            .client()?
            .custom_lists(&credentials, user_id)
            .await;
        if matches!(&result, Err(SimklError::Unauthorized)) {
            credentials = self
                .refresh_and_store_credentials(ctx, tracker, &credentials)
                .await?;
            result = self
                .client()?
                .custom_lists(&credentials, user_id)
                .await;
        }
        result
            .map_err(Self::map_error)
            .map_err(Into::into)
    }

    async fn fetch_custom_list_items(
        &self,
        ctx: &AppContext,
        tracker: &mut db::UserMediaTracker,
        list_id: i64,
    ) -> Result<Vec<SimklCustomListItem>> {
        let mut credentials = Self::parse_credentials(&tracker.credentials)?;
        let mut result = self
            .client()?
            .custom_list_items(&credentials, list_id)
            .await;
        if matches!(&result, Err(SimklError::Unauthorized)) {
            credentials = self
                .refresh_and_store_credentials(ctx, tracker, &credentials)
                .await?;
            result = self
                .client()?
                .custom_list_items(&credentials, list_id)
                .await;
        }
        result
            .map_err(Self::map_error)
            .map_err(Into::into)
    }

    async fn refresh_and_store_credentials(
        &self,
        ctx: &AppContext,
        tracker: &mut db::UserMediaTracker,
        credentials: &SimklCredentials,
    ) -> Result<SimklCredentials> {
        let refreshed = self
            .client()?
            .refresh(
                credentials
                    .refresh_token
                    .expose(),
            )
            .await
            .map_err(Self::map_error)?;
        let wrapped = Self::wrap_credentials(&refreshed)?;
        db::UserMediaTracker::replace_credentials(&ctx.db, tracker.id, &wrapped)
            .await?;
        tracker.credentials = wrapped;
        Ok(refreshed)
    }

    async fn mark_tracker_failure(
        ctx: &AppContext,
        tracker: &db::UserMediaTracker,
        error: &anyhow::Error,
    ) -> Result<()> {
        if let Some(error) = error.downcast_ref::<MediaTrackerError>() {
            db::UserMediaTracker::mark_failure(&ctx.db, tracker.id, error).await?;
        }
        Ok(())
    }

    fn tree_identity(media: &db::Media) -> Option<(i64, bool, String)> {
        let ids = if media.kind == db::MediaKind::Series {
            &media.external_ids
        } else {
            &media
                .grandparent
                .as_deref()?
                .external_ids
        };
        let raw = ids
            .custom_stremio_id
            .as_deref()?;
        let simkl_id = raw
            .strip_prefix(SIMKL_ID_PREFIX)?
            .parse()
            .ok()?;
        let anime = ids
            .custom_stremio_type
            .as_deref()
            == Some(SIMKL_ANIME_TYPE);
        Some((simkl_id, anime, ids.stremio_lookup_id()?))
    }

    fn episode_coordinates(
        episode: &SimklEpisodeDetail,
        anime: bool,
    ) -> Option<(i64, i64)> {
        if anime && let Some(tvdb) = &episode.tvdb {
            return Some((tvdb.season, tvdb.episode));
        }
        Some((
            episode
                .season
                .unwrap_or(1),
            episode.episode?,
        ))
    }
}

#[async_trait]
impl AddonKind for SimklAddon {
    fn id(&self) -> &'static str {
        "simkl"
    }
}

#[async_trait]
impl CatalogAddon for SimklAddon {
    async fn catalog_list(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let mut catalogs = vec![Self::catalog_info(
            SimklCatalogId::History,
            "Simkl History",
            db::CollectionMediaKind::Mixed,
            None,
            true,
        )];
        let trackers =
            db::UserMediaTracker::list_for_addon(&ctx.db, self.addon_id).await?;
        let mut seen_accounts = HashSet::new();
        for mut tracker in trackers
            .into_iter()
            .filter(|tracker| tracker.status == db::MediaTrackerStatus::Connected)
        {
            let account_id = Self::tracker_account_id(&tracker);
            if !seen_accounts.insert(account_id.clone()) {
                continue;
            }
            let account_name = Self::tracker_account_name(&tracker);
            catalogs.extend(
                SIMKL_STATUSES
                    .into_iter()
                    .map(|status| {
                        Self::catalog_info(
                            SimklCatalogId::Status {
                                account_id: account_id.clone(),
                                status,
                            },
                            format!(
                                "Simkl - {account_name} - {}",
                                Self::status_name(status)
                            ),
                            db::CollectionMediaKind::Mixed,
                            None,
                            false,
                        )
                    }),
            );

            let Ok(user_id) = account_id.parse::<i64>() else {
                warn!(
                    tracker_id = %tracker.id,
                    account_id,
                    "cannot discover Simkl custom lists without a numeric account id"
                );
                continue;
            };
            match self
                .fetch_custom_lists(ctx, &mut tracker, user_id)
                .await
            {
                Ok(lists) => {
                    db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
                    catalogs.extend(
                        lists
                            .iter()
                            .filter_map(|list| {
                                Self::custom_list_catalog(
                                    &account_id,
                                    &account_name,
                                    list,
                                )
                            }),
                    );
                }
                Err(error) => {
                    warn!(
                        tracker_id = %tracker.id,
                        user_id = %tracker.user_id,
                        error = %error,
                        "failed to discover Simkl custom-list catalogs"
                    );
                    Self::mark_tracker_failure(ctx, &tracker, &error).await?;
                }
            }
        }
        Ok(catalogs)
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let catalog_id = match SimklCatalogId::from_str(local_id) {
            Ok(catalog_id) => catalog_id,
            Err(_) => return Ok(None),
        };
        let mut trackers =
            db::UserMediaTracker::list_for_addon(&ctx.db, self.addon_id).await?;
        trackers.retain(|tracker| tracker.status == db::MediaTrackerStatus::Connected);

        let roots = match catalog_id {
            SimklCatalogId::History => {
                let mut roots = Vec::new();
                let mut successful = 0usize;
                let mut first_error = None;
                for mut tracker in trackers {
                    match self
                        .fetch_library(ctx, &mut tracker)
                        .await
                    {
                        Ok(library) => {
                            successful += 1;
                            db::UserMediaTracker::mark_success(&ctx.db, tracker.id)
                                .await?;
                            roots.extend(Self::library_roots(library, true));
                        }
                        Err(error) => {
                            warn!(
                                tracker_id = %tracker.id,
                                user_id = %tracker.user_id,
                                error = %error,
                                "failed to fetch Simkl history catalog"
                            );
                            Self::mark_tracker_failure(ctx, &tracker, &error).await?;
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        }
                    }
                }
                if successful == 0
                    && let Some(error) = first_error
                {
                    return Err(error);
                }
                Self::deduplicate_roots(&mut roots);
                info!(
                    connections = successful,
                    items = roots.len(),
                    "Simkl history catalog fetched"
                );
                roots
            }
            SimklCatalogId::Status { account_id, status } => {
                let Some(mut tracker) = trackers
                    .into_iter()
                    .find(|tracker| Self::tracker_account_id(tracker) == account_id)
                else {
                    anyhow::bail!("Simkl account {account_id} is not connected");
                };
                match self
                    .fetch_status_library(ctx, &mut tracker, status)
                    .await
                {
                    Ok(library) => {
                        db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
                        let roots = Self::library_roots(library, false);
                        info!(
                            account_id,
                            status = %status,
                            items = roots.len(),
                            "Simkl status catalog fetched"
                        );
                        roots
                    }
                    Err(error) => {
                        Self::mark_tracker_failure(ctx, &tracker, &error).await?;
                        return Err(error);
                    }
                }
            }
            SimklCatalogId::CustomList {
                account_id,
                list_id,
            } => {
                let Some(mut tracker) = trackers
                    .into_iter()
                    .find(|tracker| Self::tracker_account_id(tracker) == account_id)
                else {
                    anyhow::bail!("Simkl account {account_id} is not connected");
                };
                match self
                    .fetch_custom_list_items(ctx, &mut tracker, list_id)
                    .await
                {
                    Ok(items) => {
                        db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
                        let roots = Self::custom_list_roots(items);
                        info!(
                            account_id,
                            list_id,
                            items = roots.len(),
                            "Simkl custom-list catalog fetched"
                        );
                        roots
                    }
                    Err(error) => {
                        Self::mark_tracker_failure(ctx, &tracker, &error).await?;
                        return Err(error);
                    }
                }
            }
        };
        Ok(Some(Box::pin(stream::iter(roots))))
    }
}

#[async_trait]
impl MediaTrackerAddon for SimklAddon {
    fn capabilities(&self) -> MediaTrackerCapabilities {
        MediaTrackerCapabilities {
            auth_flow: AuthFlow::OAuthDeviceCode,
            history_import: false,
            ..Default::default()
        }
    }

    fn configured(&self) -> bool {
        self.client
            .is_some()
    }

    async fn begin_device_auth(
        &self,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<DeviceAuthStart> {
        let code = self
            .client()?
            .begin_device_auth()
            .await
            .map_err(Self::map_error)?;
        Ok(DeviceAuthStart {
            verification_url: code
                .verification_uri_complete
                .unwrap_or(code.verification_uri),
            user_code: code.user_code,
            poll_token: code
                .device_code
                .into_inner(),
            interval: Duration::from_secs(
                code.interval
                    .max(1),
            ),
            expires_in: Duration::from_secs(code.expires_in),
        })
    }

    async fn poll_device_auth(
        &self,
        poll_token: &str,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<DeviceAuthPoll> {
        match self
            .client()?
            .poll_device_token(poll_token)
            .await
            .map_err(Self::map_error)?
        {
            DeviceTokenResponse::Waiting(DeviceTokenPoll::Pending) => {
                Ok(DeviceAuthPoll::Pending)
            }
            DeviceTokenResponse::Waiting(DeviceTokenPoll::SlowDown) => {
                Ok(DeviceAuthPoll::SlowDown)
            }
            DeviceTokenResponse::Waiting(DeviceTokenPoll::Expired) => {
                Ok(DeviceAuthPoll::Expired)
            }
            DeviceTokenResponse::Approved(token) => {
                let credentials: SimklCredentials = token.into();
                let settings = self
                    .client()?
                    .settings(&credentials)
                    .await
                    .map_err(Self::map_error)?;
                Ok(DeviceAuthPoll::Approved(MediaTrackerConnection {
                    credentials: Self::wrap_credentials(&credentials)?,
                    remote_account_id: Some(
                        settings
                            .account
                            .id
                            .to_string(),
                    ),
                    remote_account_name: Some(
                        settings
                            .user
                            .name,
                    ),
                }))
            }
        }
    }

    async fn refresh(
        &self,
        credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<MediaTrackerCredentials> {
        let credentials = Self::parse_credentials(credentials)?;
        let refreshed = self
            .client()?
            .refresh(
                credentials
                    .refresh_token
                    .expose(),
            )
            .await
            .map_err(Self::map_error)?;
        Self::wrap_credentials(&refreshed)
    }

    async fn verify(
        &self,
        credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let credentials = Self::parse_credentials(credentials)?;
        self.client()?
            .settings(&credentials)
            .await
            .map(|_| ())
            .map_err(Self::map_error)
    }

    async fn disconnect(
        &self,
        credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let credentials = Self::parse_credentials(credentials)?;
        self.client()?
            .revoke(
                credentials
                    .refresh_token
                    .expose(),
            )
            .await
            .map_err(Self::map_error)
    }

    async fn on_event(
        &self,
        _event: &MediaTrackerEvent,
        _target: &MediaTrackerTarget,
        _credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        Err(MediaTrackerError::unsupported(
            "Simkl event delivery is handled by Crosswatch",
        ))
    }
}

#[async_trait]
impl MetaAddon for SimklAddon {
    async fn supports(&self, _media: &db::Media) -> bool {
        false
    }

    async fn meta_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
        _config: &api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        Ok(None)
    }

    fn on_series_done(&self, meta_id: &str) {
        self.episode_cache
            .lock()
            .unwrap()
            .remove(meta_id);
    }
}

#[async_trait]
impl TreeAddon for SimklAddon {
    fn supports(&self, root: &db::Media) -> bool {
        matches!(root.kind, db::MediaKind::Series | db::MediaKind::Season)
            && Self::tree_identity(root).is_some()
            && self
                .client
                .is_some()
    }

    async fn get_children(
        &self,
        root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        let (simkl_id, anime, cache_key) = match Self::tree_identity(root) {
            Some(identity) => identity,
            None => return Ok(None),
        };
        match root.kind {
            db::MediaKind::Series => {
                let episodes = Arc::new(
                    self.client()?
                        .episodes(simkl_id, anime)
                        .await
                        .map_err(Self::map_error)?,
                );
                let series_key = root.series_canonical_key();
                let mut season_numbers = episodes
                    .iter()
                    .filter_map(|episode| Self::episode_coordinates(episode, anime))
                    .map(|(season, _)| season)
                    .collect::<Vec<_>>();
                season_numbers.sort_unstable();
                season_numbers.dedup();
                self.episode_cache
                    .lock()
                    .unwrap()
                    .insert(cache_key, episodes);
                let seasons = season_numbers
                    .into_iter()
                    .map(|number| db::Media {
                        id: db::Media::season_id(&series_key, number),
                        title: if number == 0 {
                            "Specials".into()
                        } else {
                            format!("Season {number}")
                        },
                        kind: db::MediaKind::Season,
                        idx: Some(number),
                        parent_id: Some(root.id),
                        grandparent_id: Some(root.id),
                        refreshed_at: Some(chrono::Utc::now().naive_utc()),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                Ok((!seasons.is_empty()).then_some(seasons))
            }
            db::MediaKind::Season => {
                let Some(season_number) = root.idx else {
                    return Ok(None);
                };
                let episodes = self
                    .episode_cache
                    .lock()
                    .unwrap()
                    .get(&cache_key)
                    .cloned();
                let Some(episodes) = episodes else {
                    return Ok(None);
                };
                let Some(grandparent) = root
                    .grandparent
                    .as_deref()
                else {
                    return Ok(None);
                };
                let series_key = grandparent.series_canonical_key();
                let children = episodes
                    .iter()
                    .filter_map(|episode| {
                        let (season, number) =
                            Self::episode_coordinates(episode, anime)?;
                        (season == season_number).then(|| db::Media {
                            id: db::Media::episode_id(&series_key, season, number),
                            title: episode
                                .title
                                .clone(),
                            kind: db::MediaKind::Episode,
                            description: episode
                                .description
                                .clone(),
                            released_at: episode
                                .date
                                .map(|date| date.naive_utc()),
                            idx: Some(number),
                            parent_idx: Some(season),
                            parent_id: Some(root.id),
                            grandparent_id: root.grandparent_id,
                            refreshed_at: Some(chrono::Utc::now().naive_utc()),
                            ..Default::default()
                        })
                    })
                    .collect::<Vec<_>>();
                Ok((!children.is_empty()).then_some(children))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_entry(title: &str, simkl_id: i64, watched: bool) -> SimklLibraryEntry {
        SimklLibraryEntry {
            last_watched_at: watched.then(chrono::Utc::now),
            status: remux_sdks::simkl::SimklStatus::Watching,
            user_rating: None,
            show: None,
            movie: Some(SimklMedia {
                title: title.into(),
                year: Some(2024),
                ids: SimklItemIds {
                    simkl: Some(simkl_id),
                    ..Default::default()
                },
            }),
            anime_type: None,
            seasons: vec![],
        }
    }

    #[test]
    fn simkl_ids_keep_known_ids_and_a_stable_fallback() {
        let ids = SimklAddon::ids(
            &SimklItemIds {
                simkl: Some(53536),
                imdb: Some("tt0181852".into()),
                tmdb: Some("296".into()),
                ..Default::default()
            },
            SIMKL_MOVIE_TYPE,
        );

        assert_eq!(
            ids.imdb
                .as_ref()
                .map(ToString::to_string),
            Some("tt0181852".to_string())
        );
        assert_eq!(ids.tmdb, Some(296));
        assert_eq!(
            ids.custom_stremio_id
                .as_deref(),
            Some("simkl:53536")
        );
    }

    #[test]
    fn anime_prefers_tvdb_episode_coordinates() {
        let episode = SimklEpisodeDetail {
            title: "Cruelty".into(),
            description: None,
            season: None,
            episode: Some(1),
            kind: "episode".into(),
            aired: Some(true),
            date: None,
            ids: SimklItemIds::default(),
            tvdb: Some(remux_sdks::simkl::SimklEpisodeCoordinate {
                season: 3,
                episode: 7,
            }),
        };

        assert_eq!(
            SimklAddon::episode_coordinates(&episode, true),
            Some((3, 7))
        );
        assert_eq!(
            SimklAddon::episode_coordinates(&episode, false),
            Some((1, 1))
        );
    }

    #[test]
    fn history_catalog_only_materializes_watched_roots_and_deduplicates_them() {
        let watched = library_entry("Watched", 42, true);
        let duplicate = watched.clone();
        let unwatched = library_entry("Plan to watch", 43, false);
        let mut completed = library_entry("Completed", 44, false);
        completed.status = SimklStatus::Completed;
        let roots = SimklAddon::library_roots(
            remux_sdks::simkl::SimklLibrary {
                movies: vec![watched, duplicate, unwatched, completed],
                ..Default::default()
            },
            true,
        );

        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].title, "Watched");
        assert_eq!(roots[0].kind, db::MediaKind::Movie);
        assert_eq!(
            roots[0]
                .external_ids
                .custom_stremio_id
                .as_deref(),
            Some("simkl:42")
        );
    }

    #[test]
    fn status_catalog_materializes_unwatched_roots() {
        let roots = SimklAddon::library_roots(
            SimklLibrary {
                movies: vec![library_entry("Plan to watch", 43, false)],
                ..Default::default()
            },
            false,
        );

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].title, "Plan to watch");
    }

    #[test]
    fn catalog_ids_round_trip() {
        let status = SimklCatalogId::Status {
            account_id: "123".into(),
            status: SimklStatus::Plantowatch,
        };
        let list = SimklCatalogId::CustomList {
            account_id: "123".into(),
            list_id: 456,
        };

        assert_eq!(
            status
                .to_string()
                .parse::<SimklCatalogId>()
                .unwrap(),
            status
        );
        assert_eq!(
            list.to_string()
                .parse::<SimklCatalogId>()
                .unwrap(),
            list
        );
    }

    #[test]
    fn anime_movie_in_custom_list_is_a_movie() {
        let media = SimklAddon::custom_list_media(&SimklCustomListItem {
            title: "Akira".into(),
            year: Some(1988),
            kind: SimklMediaType::Anime,
            ids: SimklItemIds {
                simkl: Some(123),
                ..Default::default()
            },
            anime_type: Some(SimklAnimeType::Movie),
            release_date: None,
            overview: Some("Neo-Tokyo".into()),
        });

        assert_eq!(media.kind, db::MediaKind::Movie);
        assert_eq!(
            media
                .description
                .as_deref(),
            Some("Neo-Tokyo")
        );
        assert_eq!(
            media
                .external_ids
                .custom_stremio_type
                .as_deref(),
            Some(SIMKL_MOVIE_TYPE)
        );
    }
}
