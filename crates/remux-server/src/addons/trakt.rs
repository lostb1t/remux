use std::{sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use remux_sdks::trakt::{
    DeviceTokenPoll, DeviceTokenResponse, TraktClient, TraktCredentials, TraktError,
    TraktItemIds, TraktScrobbleAction,
};
use remux_utils::Secret;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    MediaKind,
    media_tracker::{
        AuthFlow, DeviceAuthPoll, DeviceAuthStart, MediaTrackerAddon,
        MediaTrackerCapabilities, MediaTrackerConnection, MediaTrackerCredentials,
        MediaTrackerCtx, MediaTrackerError, MediaTrackerEvent, MediaTrackerEventKind,
        MediaTrackerResult, MediaTrackerTarget, RemoteProgress, RemoteWatch,
        SyncDirection,
    },
};
use crate::db;

pub struct TraktPreset;

impl AddonPreset for TraktPreset {
    fn id(&self) -> &'static str {
        "trakt"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "trakt".into(),
            display_name: "Trakt".into(),
            description: "Imports watch history and scrobbles playback to Trakt."
                .into(),
            icon: None,
            supported_resources: vec![],
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
        _addon_id: Uuid,
        _cfg: &Value,
        config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let client = config
            .trakt_client_id
            .as_ref()
            .zip(
                config
                    .trakt_client_secret
                    .as_ref(),
            )
            .filter(|(client_id, client_secret)| {
                !client_id
                    .trim()
                    .is_empty()
                    && !client_secret
                        .expose()
                        .trim()
                        .is_empty()
            })
            .map(|(client_id, client_secret)| {
                TraktClient::new(
                    &config.trakt_api_base_url,
                    client_id,
                    client_secret.clone(),
                )
            });
        let addon = Arc::new(TraktAddon { client });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            media_tracker: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TraktPreset))
}

pub struct TraktAddon {
    client: Option<TraktClient>,
}

impl TraktAddon {
    fn client(&self) -> MediaTrackerResult<&TraktClient> {
        self.client.as_ref().ok_or_else(|| {
            MediaTrackerError::permanent(
                "Trakt is not configured; set trakt_client_id and trakt_client_secret",
            )
        })
    }

    fn parse_credentials(
        credentials: &MediaTrackerCredentials,
    ) -> MediaTrackerResult<TraktCredentials> {
        serde_json::from_value(
            credentials
                .expose()
                .clone(),
        )
        .map_err(|_| MediaTrackerError::reauth("stored Trakt credentials are invalid"))
    }

    fn wrap_credentials(
        credentials: &TraktCredentials,
    ) -> MediaTrackerResult<MediaTrackerCredentials> {
        serde_json::to_value(credentials)
            .map(Secret::new)
            .map_err(|e| {
                MediaTrackerError::permanent(format!(
                    "could not store Trakt credentials: {e}"
                ))
            })
    }

    fn map_error(error: TraktError) -> MediaTrackerError {
        match error {
            TraktError::Unauthorized => {
                MediaTrackerError::reauth("Trakt authorization is invalid or expired")
            }
            TraktError::RateLimited {
                retry_after: Some(after),
            } => MediaTrackerError::retry_after("Trakt rate limit reached", after),
            other if other.is_retryable() => {
                MediaTrackerError::retryable(other.to_string())
            }
            other => MediaTrackerError::permanent(other.to_string()),
        }
    }

    fn ids(ids: &TraktItemIds) -> db::ExternalIds {
        db::ExternalIds {
            imdb: ids
                .imdb
                .as_ref()
                .and_then(|id| db::NonEmptyString::try_new(id.clone()).ok()),
            tmdb: ids.tmdb,
            tvdb: ids.tvdb,
            ..Default::default()
        }
    }

    fn payload_ids(ids: &db::ExternalIds) -> Value {
        let mut values = serde_json::Map::new();
        if let Some(imdb) = ids
            .imdb
            .as_deref()
        {
            values.insert("imdb".into(), json!(imdb));
        }
        if let Some(tmdb) = ids.tmdb {
            values.insert("tmdb".into(), json!(tmdb));
        }
        if let Some(tvdb) = ids.tvdb {
            values.insert("tvdb".into(), json!(tvdb));
        }
        Value::Object(values)
    }

    fn target_payload(target: &MediaTrackerTarget) -> MediaTrackerResult<Value> {
        match target.kind {
            db::MediaKind::Movie => Ok(json!({
                "movie": {
                    "title": target.title,
                    "year": target.year,
                    "ids": Self::payload_ids(&target.ids),
                }
            })),
            db::MediaKind::Episode => {
                let series = target
                    .series
                    .as_deref()
                    .ok_or_else(|| {
                        MediaTrackerError::permanent("episode has no series identity")
                    })?;
                Ok(json!({
                    "episode": {
                        "season": target.season,
                        "number": target.episode,
                        "ids": Self::payload_ids(&target.ids),
                    },
                    "show": {
                        "title": series.title,
                        "year": series.year,
                        "ids": Self::payload_ids(&series.ids),
                    }
                }))
            }
            _ => Err(MediaTrackerError::permanent(
                "Trakt only supports movies and episodes",
            )),
        }
    }

    fn progress(target: &MediaTrackerTarget, position_ticks: i64) -> f32 {
        let Some(runtime) = target
            .runtime_ticks
            .filter(|runtime| *runtime > 0)
        else {
            return 0.0;
        };
        ((position_ticks.max(0) as f64 / runtime as f64) * 100.0).clamp(0.0, 100.0)
            as f32
    }

    fn should_send_stop(progress: f32) -> bool {
        progress >= 1.0
    }

    fn history_payload(
        target: &MediaTrackerTarget,
        watched_at: Option<chrono::DateTime<Utc>>,
    ) -> MediaTrackerResult<Value> {
        Ok(match target.kind {
            db::MediaKind::Movie => json!({
                "movies": [{
                    "title": target.title,
                    "year": target.year,
                    "ids": Self::payload_ids(&target.ids),
                    "watched_at": watched_at,
                }]
            }),
            db::MediaKind::Episode => {
                let series = target
                    .series
                    .as_deref()
                    .ok_or_else(|| {
                        MediaTrackerError::permanent("episode has no series identity")
                    })?;
                json!({
                    "shows": [{
                        "title": series.title,
                        "year": series.year,
                        "ids": Self::payload_ids(&series.ids),
                        "seasons": [{
                            "number": target.season,
                            "episodes": [{
                                "number": target.episode,
                                "watched_at": watched_at,
                            }]
                        }]
                    }]
                })
            }
            _ => {
                return Err(MediaTrackerError::permanent(
                    "Trakt history updates only support movies and episodes",
                ));
            }
        })
    }
}

#[async_trait]
impl AddonKind for TraktAddon {
    fn id(&self) -> &'static str {
        "trakt"
    }
}

#[async_trait]
impl MediaTrackerAddon for TraktAddon {
    fn capabilities(&self) -> MediaTrackerCapabilities {
        let events = vec![
            MediaTrackerEventKind::PlaybackStart,
            MediaTrackerEventKind::PlaybackProgress,
            MediaTrackerEventKind::PlaybackStop,
            MediaTrackerEventKind::MarkPlayed,
            MediaTrackerEventKind::MarkUnplayed,
        ];
        MediaTrackerCapabilities {
            auth_flow: AuthFlow::OAuthDeviceCode,
            supported_events: events.clone(),
            default_event_filter: events,
            history_import: true,
            progress_import: true,
            watch_state_sync: SyncDirection::Both,
            ..Default::default()
        }
    }

    fn history_catalog_tag(&self) -> Option<&'static str> {
        Some("source:Trakt")
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
            verification_url: code.verification_url,
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
            DeviceTokenResponse::Waiting(DeviceTokenPoll::Denied) => {
                Ok(DeviceAuthPoll::Denied)
            }
            DeviceTokenResponse::Waiting(DeviceTokenPoll::Expired) => {
                Ok(DeviceAuthPoll::Expired)
            }
            DeviceTokenResponse::Approved(token) => {
                let credentials: TraktCredentials = token.into();
                let account = self
                    .client()?
                    .settings(&credentials)
                    .await
                    .map_err(Self::map_error)?
                    .user;
                Ok(DeviceAuthPoll::Approved(MediaTrackerConnection {
                    credentials: Self::wrap_credentials(&credentials)?,
                    remote_account_id: account
                        .ids
                        .trakt
                        .map(|id| id.to_string())
                        .or(account
                            .ids
                            .slug),
                    remote_account_name: Some(account.username),
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
                    .access_token
                    .expose(),
            )
            .await
            .map_err(Self::map_error)
    }

    async fn on_event(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let credentials = Self::parse_credentials(credentials)?;
        match event {
            MediaTrackerEvent::PlaybackStart { position_ticks, .. } => {
                let mut payload = Self::target_payload(target)?;
                payload["progress"] = json!(Self::progress(target, *position_ticks));
                self.client()?
                    .scrobble(TraktScrobbleAction::Start, &payload, &credentials)
                    .await
                    .map_err(Self::map_error)
            }
            MediaTrackerEvent::PlaybackProgress {
                position_ticks,
                is_paused,
                ..
            } => {
                let mut payload = Self::target_payload(target)?;
                payload["progress"] = json!(Self::progress(target, *position_ticks));
                self.client()?
                    .scrobble(
                        if *is_paused {
                            TraktScrobbleAction::Pause
                        } else {
                            TraktScrobbleAction::Start
                        },
                        &payload,
                        &credentials,
                    )
                    .await
                    .map_err(Self::map_error)
            }
            MediaTrackerEvent::PlaybackStop { position_ticks, .. } => {
                let progress = Self::progress(target, *position_ticks);
                // Trakt treats a stop below its completion threshold as a pause,
                // but rejects pause progress below 1%. Nothing meaningful has
                // been watched yet, so acknowledge the event without sending it.
                if !Self::should_send_stop(progress) {
                    return Ok(());
                }
                let mut payload = Self::target_payload(target)?;
                payload["progress"] = json!(progress);
                self.client()?
                    .scrobble(TraktScrobbleAction::Stop, &payload, &credentials)
                    .await
                    .map_err(Self::map_error)
            }
            MediaTrackerEvent::MarkPlayed => self
                .client()?
                .history(
                    false,
                    &Self::history_payload(target, Some(Utc::now()))?,
                    &credentials,
                )
                .await
                .map_err(Self::map_error),
            MediaTrackerEvent::MarkUnplayed => self
                .client()?
                .history(true, &Self::history_payload(target, None)?, &credentials)
                .await
                .map_err(Self::map_error),
            _ => Err(MediaTrackerError::unsupported("this Trakt event")),
        }
    }

    async fn import_history(
        &self,
        credentials: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<Vec<RemoteWatch>> {
        let credentials = Self::parse_credentials(credentials)?;
        let client = self.client()?;
        let (movies, shows, paused_movies, paused_episodes) = tokio::try_join!(
            client.watched_movies(&credentials),
            client.watched_shows(&credentials),
            client.movie_playback(&credentials),
            client.episode_playback(&credentials),
        )
        .map_err(Self::map_error)?;

        let mut watches = Vec::new();
        watches.extend(
            movies
                .into_iter()
                .map(|entry| RemoteWatch {
                    title: entry
                        .movie
                        .title
                        .clone(),
                    year: entry
                        .movie
                        .year,
                    ids: Self::ids(
                        &entry
                            .movie
                            .ids,
                    ),
                    season: None,
                    episode: None,
                    watched: entry.plays > 0,
                    play_count: Some(entry.plays),
                    progress: None,
                    watched_at: entry
                        .last_watched_at
                        .map(|at| at.naive_utc()),
                    progress_at: None,
                    favorite: None,
                    rating: None,
                }),
        );
        for entry in shows {
            let ids = Self::ids(
                &entry
                    .show
                    .ids,
            );
            for season in entry.seasons {
                for episode in season.episodes {
                    watches.push(RemoteWatch {
                        title: entry
                            .show
                            .title
                            .clone(),
                        year: entry
                            .show
                            .year,
                        ids: ids.clone(),
                        season: Some(season.number),
                        episode: Some(episode.number),
                        watched: episode.plays > 0,
                        play_count: Some(episode.plays),
                        progress: None,
                        watched_at: episode
                            .last_watched_at
                            .map(|at| at.naive_utc()),
                        progress_at: None,
                        favorite: None,
                        rating: None,
                    });
                }
            }
        }
        watches.extend(
            paused_movies
                .into_iter()
                .map(|entry| RemoteWatch {
                    title: entry
                        .movie
                        .title
                        .clone(),
                    year: entry
                        .movie
                        .year,
                    ids: Self::ids(
                        &entry
                            .movie
                            .ids,
                    ),
                    season: None,
                    episode: None,
                    watched: false,
                    play_count: None,
                    progress: Some(RemoteProgress::Percent(entry.progress)),
                    watched_at: None,
                    progress_at: Some(
                        entry
                            .paused_at
                            .naive_utc(),
                    ),
                    favorite: None,
                    rating: None,
                }),
        );
        watches.extend(
            paused_episodes
                .into_iter()
                .map(|entry| RemoteWatch {
                    title: entry
                        .show
                        .title
                        .clone(),
                    year: entry
                        .show
                        .year,
                    ids: Self::ids(
                        &entry
                            .show
                            .ids,
                    ),
                    season: Some(
                        entry
                            .episode
                            .season,
                    ),
                    episode: Some(
                        entry
                            .episode
                            .number,
                    ),
                    watched: false,
                    play_count: None,
                    progress: Some(RemoteProgress::Percent(entry.progress)),
                    watched_at: None,
                    progress_at: Some(
                        entry
                            .paused_at
                            .naive_utc(),
                    ),
                    favorite: None,
                    rating: None,
                }),
        );
        Ok(watches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_clamped_and_uses_jellyfin_ticks() {
        let target = MediaTrackerTarget {
            kind: db::MediaKind::Movie,
            title: "Movie".into(),
            year: None,
            ids: db::ExternalIds::default(),
            series: None,
            season: None,
            episode: None,
            runtime_ticks: Some(1_000),
        };
        assert_eq!(TraktAddon::progress(&target, 250), 25.0);
        assert_eq!(TraktAddon::progress(&target, 2_000), 100.0);
        assert_eq!(TraktAddon::progress(&target, -1), 0.0);
    }

    #[test]
    fn stop_requires_one_percent_progress() {
        assert!(!TraktAddon::should_send_stop(0.0));
        assert!(!TraktAddon::should_send_stop(0.99));
        assert!(TraktAddon::should_send_stop(1.0));
    }

    #[test]
    fn episode_history_falls_back_to_show_season_and_number() {
        let target = MediaTrackerTarget {
            kind: db::MediaKind::Episode,
            title: "Pilot".into(),
            year: None,
            ids: db::ExternalIds::default(),
            series: Some(Box::new(MediaTrackerTarget {
                kind: db::MediaKind::Series,
                title: "Example Show".into(),
                year: Some(2024),
                ids: db::ExternalIds {
                    tvdb: Some(42),
                    ..Default::default()
                },
                series: None,
                season: None,
                episode: None,
                runtime_ticks: None,
            })),
            season: Some(1),
            episode: Some(2),
            runtime_ticks: Some(30 * 60 * 10_000_000),
        };
        let payload = TraktAddon::history_payload(&target, None).unwrap();
        assert_eq!(payload["shows"][0]["ids"]["tvdb"], 42);
        assert_eq!(payload["shows"][0]["seasons"][0]["number"], 1);
        assert_eq!(
            payload["shows"][0]["seasons"][0]["episodes"][0]["number"],
            2
        );
    }

    #[test]
    fn history_payload_rejects_series_without_panicking() {
        let target = MediaTrackerTarget {
            kind: db::MediaKind::Series,
            title: "Example Show".into(),
            year: Some(2024),
            ids: db::ExternalIds::default(),
            series: None,
            season: None,
            episode: None,
            runtime_ticks: None,
        };

        let error = TraktAddon::history_payload(&target, None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Trakt history updates only support movies and episodes"
        );
        assert!(!error.is_retryable());
    }
}
