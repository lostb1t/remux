//! Yamtrack, reached through its Jellyfin webhook.
//!
//! Yamtrack has no write API, so the webhook its Jellyfin integration exposes
//! is the only way in. It accepts four events and reads nothing off an item but
//! its external ids, its type, and a played flag. Scores and favourites are
//! things Yamtrack keeps and this route cannot set, which is why the
//! capabilities claim neither.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, MediaKind, ResourceType,
    media_tracker::{
        AuthFlow, MediaTrackerAddon, MediaTrackerCapabilities, MediaTrackerCredentials,
        MediaTrackerCtx, MediaTrackerError, MediaTrackerEvent, MediaTrackerEventKind,
        MediaTrackerResult, MediaTrackerTarget,
    },
    webhook_media_tracker::{
        NotificationType, WebhookFormat, WebhookItem, post, provider_ids,
    },
};
use crate::db;

pub struct YamtrackPreset;

impl AddonPreset for YamtrackPreset {
    fn id(&self) -> &'static str {
        "yamtrack"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "yamtrack".to_string(),
            display_name: "Yamtrack".to_string(),
            description: "Yamtrack, a self-hosted media tracker.".to_string(),
            icon: None,
            supported_resources: vec![AddonMetadata::simple_resource(
                ResourceType::Tracking,
            )],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
            supported_resources_user: vec![ResourceType::Tracking],
            supported_types_user: vec![MediaKind::Movie, MediaKind::Series],
            options: vec![AddonOption {
                id: "base_url".to_string(),
                name: "Yamtrack URL".to_string(),
                description: Some(
                    "Where your Yamtrack lives, for example https://yamtrack.example.com."
                        .to_string(),
                ),
                required: true,
                default: None,
                kind: AddonOptionType::Url,
            }],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        // Erroring here drops the addon from the runtime list, so an
        // unconfigured Yamtrack offers no tracking rather than accepting
        // connections it could never deliver for.
        let base_url = cfg
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("yamtrack: base_url is not set"))?
            .trim_end_matches('/')
            .to_string();

        Ok(AddonCapabilities {
            media_tracker: Some(Arc::new(YamtrackAddon {
                base_url,
                client: super::make_http_client(),
            })),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(YamtrackPreset))
}

pub struct YamtrackAddon {
    base_url: String,
    client: reqwest::Client,
}

impl AddonKind for YamtrackAddon {
    fn id(&self) -> &'static str {
        "yamtrack"
    }
}

impl YamtrackAddon {
    /// Not the route the backend declares: the token is a path segment on
    /// Yamtrack's Jellyfin webhook, not a header or query parameter.
    fn webhook_url(&self, token: &str) -> String {
        format!("{}/webhook/jellyfin/{}", self.base_url, token)
    }

    fn token(creds: &MediaTrackerCredentials) -> MediaTrackerResult<&str> {
        creds
            .get_str("token")
            .ok_or_else(|| MediaTrackerError::reauth("no webhook token stored"))
    }
}

/// Yamtrack's reading of a Jellyfin notification. It resolves an episode from
/// the episode's own TVDB or IMDB id, not from the series plus numbers, so the
/// item always carries its own ids rather than [`WebhookItem::matching_ids`].
pub struct YamtrackFormat;

impl WebhookFormat for YamtrackFormat {
    fn body(&self, item: &WebhookItem) -> Value {
        let event = match item.notification_type {
            NotificationType::PlaybackStart => "Play",
            // Unreachable: `capabilities` does not declare PlaybackProgress,
            // so `enqueue` never queues one for this tracker.
            NotificationType::PlaybackProgress => "Play",
            NotificationType::PlaybackStop => "Stop",
            NotificationType::UserDataSaved => {
                if item.played_to_completion {
                    "MarkPlayed"
                } else {
                    "MarkUnplayed"
                }
            }
        };

        let mut inner = json!({
            "Type": if item.kind == db::MediaKind::Episode { "Episode" } else { "Movie" },
            "Name": item.name,
            "ProviderIds": provider_ids(&item.ids),
            "UserData": { "Played": item.played_to_completion },
        });
        let map = inner
            .as_object_mut()
            .expect("built as an object");

        if item.kind == db::MediaKind::Episode {
            if let Some(series_name) = &item.series_name {
                map.insert("SeriesName".into(), json!(series_name));
            }
            if let Some(season) = item.season {
                map.insert("ParentIndexNumber".into(), json!(season));
            }
            if let Some(episode) = item.episode {
                map.insert("IndexNumber".into(), json!(episode));
            }
        } else if let Some(year) = item.year {
            map.insert("ProductionYear".into(), json!(year));
        }

        json!({
            "Event": event,
            "Item": inner,
        })
    }
}

#[async_trait]
impl MediaTrackerAddon for YamtrackAddon {
    /// Ratings are absent on purpose. Yamtrack keeps them, but its only way in
    /// is the Jellyfin webhook, which carries an event and an item and has no
    /// field for a score. The four events below are every one that webhook
    /// accepts, so this is the whole of what Yamtrack can be told.
    fn capabilities(&self) -> MediaTrackerCapabilities {
        MediaTrackerCapabilities {
            auth_flow: AuthFlow::Token,
            connect_fields: vec![AddonOption {
                id: "token".to_string(),
                name: "Webhook token".to_string(),
                description: Some(
                    "From Yamtrack under Settings, Jellyfin. Mark played and mark \
                     unplayed also have to be enabled there before Yamtrack acts on \
                     them."
                        .to_string(),
                ),
                required: true,
                default: None,
                kind: AddonOptionType::Password,
            }],
            supported_events: vec![
                MediaTrackerEventKind::PlaybackStart,
                MediaTrackerEventKind::PlaybackStop,
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::MarkUnplayed,
            ],
            default_event_filter: vec![
                MediaTrackerEventKind::PlaybackStop,
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::MarkUnplayed,
            ],
            ..Default::default()
        }
    }

    async fn connect_with_token(
        &self,
        fields: &Value,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<MediaTrackerCredentials> {
        let token = fields
            .get("token")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                MediaTrackerError::permanent("a webhook token is required")
            })?;

        // An empty body carries no event, so Yamtrack answers the token and
        // nothing else. Anything with an `Event` would be a real scrobble.
        post(&self.client, &self.webhook_url(token), &json!({})).await?;
        Ok(MediaTrackerCredentials::new(json!({ "token": token })))
    }

    async fn verify(
        &self,
        creds: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        post(
            &self.client,
            &self.webhook_url(Self::token(creds)?),
            &json!({}),
        )
        .await
    }

    async fn on_event(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        creds: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let item = WebhookItem::from_event(event, target).ok_or_else(|| {
            MediaTrackerError::unsupported(
                &event
                    .kind()
                    .to_string(),
            )
        })?;
        post(
            &self.client,
            &self.webhook_url(Self::token(creds)?),
            &YamtrackFormat.body(&item),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn addon(server: &MockServer) -> YamtrackAddon {
        YamtrackAddon {
            base_url: server
                .base_url()
                .trim_end_matches('/')
                .to_string(),
            client: super::super::make_http_client(),
        }
    }

    fn ctx() -> MediaTrackerCtx {
        MediaTrackerCtx {
            config: Arc::new(crate::Config::default()),
        }
    }

    fn creds() -> MediaTrackerCredentials {
        MediaTrackerCredentials::new(json!({ "token": "tok" }))
    }

    fn movie() -> MediaTrackerTarget {
        MediaTrackerTarget {
            kind: db::MediaKind::Movie,
            title: "Heat".into(),
            year: Some(1995),
            ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0113277".to_string()).ok(),
                tmdb: Some(949),
                ..Default::default()
            },
            series: None,
            season: None,
            episode: None,
        }
    }

    fn episode() -> MediaTrackerTarget {
        MediaTrackerTarget {
            kind: db::MediaKind::Episode,
            title: "The Target".into(),
            year: None,
            ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0749419".to_string()).ok(),
                tmdb: Some(972467),
                tvdb: Some(303821),
                ..Default::default()
            },
            series: Some(Box::new(MediaTrackerTarget {
                kind: db::MediaKind::Series,
                title: "The Wire".into(),
                year: Some(2002),
                ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new("tt0306414".to_string()).ok(),
                    tvdb: Some(79126),
                    ..Default::default()
                },
                series: None,
                season: None,
                episode: None,
            })),
            season: Some(1),
            episode: Some(1),
        }
    }

    #[tokio::test]
    async fn a_transport_failure_does_not_put_the_token_in_the_error() {
        // `last_error` is stored on the connection and returned by the API, so
        // a url-bearing error would hand the webhook token to any caller who
        // can read the connection.
        let addon = YamtrackAddon {
            base_url: "http://127.0.0.1:1".to_string(),
            client: reqwest::Client::new(),
        };
        let err = addon
            .on_event(
                &MediaTrackerEvent::MarkPlayed,
                &movie(),
                &MediaTrackerCredentials::new(
                    json!({ "token": "s3cr3t-webhook-token" }),
                ),
                &ctx(),
            )
            .await
            .expect_err("nothing is listening on port 1");

        let rendered = format!("{err} {err:?}");
        assert!(
            !rendered.contains("s3cr3t-webhook-token"),
            "the token leaked into the error: {rendered}"
        );
    }

    #[tokio::test]
    async fn a_finished_movie_posts_a_played_stop() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/tok")
                .json_body(json!({
                    "Event": "Stop",
                    "Item": {
                        "Type": "Movie",
                        "Name": "Heat",
                        "ProductionYear": 1995,
                        "ProviderIds": { "Tmdb": "949", "Imdb": "tt0113277" },
                        "UserData": { "Played": true },
                    },
                }));
            then.status(200);
        });

        addon(&server)
            .on_event(
                &MediaTrackerEvent::PlaybackStop {
                    position_ticks: 0,
                    played: true,
                },
                &movie(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        mock.assert();
    }

    #[tokio::test]
    async fn an_episode_is_identified_by_its_own_ids() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/tok")
                .json_body(json!({
                    "Event": "MarkPlayed",
                    "Item": {
                        "Type": "Episode",
                        "Name": "The Target",
                        "SeriesName": "The Wire",
                        "ParentIndexNumber": 1,
                        "IndexNumber": 1,
                        "ProviderIds": {
                            "Tmdb": "972467",
                            "Imdb": "tt0749419",
                            "Tvdb": "303821",
                        },
                        "UserData": { "Played": true },
                    },
                }));
            then.status(200);
        });

        addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &episode(), &creds(), &ctx())
            .await
            .unwrap();

        mock.assert();
    }

    /// The ids Yamtrack matches an episode on are not on the row at ingest:
    /// TMDB's season listing carries no `external_ids`, so the episode reaches
    /// delivery with nothing of its own. Nothing in this file fills them, so a
    /// change to `complete_episode_ids` would leave Yamtrack taking a 200 for
    /// an episode it cannot place.
    #[tokio::test]
    async fn the_delivery_path_fills_in_the_ids_this_format_matches_on() {
        let tmdb = MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/find/tt5550007");
            then.status(200)
                .json_body(json!({
                    "tv_results": [{ "id": 5554439, "name": "The Wire" }],
                    "movie_results": []
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/tv/5554439/season/1/episode/1");
            then.status(200)
                .json_body(json!({
                    "id": 972467,
                    "name": "The Target",
                    "season_number": 1,
                    "episode_number": 1,
                    "external_ids": { "imdb_id": "tt0749419", "tvdb_id": 303821 },
                }));
        });
        let guard =
            crate::integration_test::new_test_server_with_config(crate::Config {
                database_url: Some("sqlite::memory:".into()),
                torrent_http_port: None,
                disable_dht: true,
                tmdb_base_url: tmdb.base_url(),
                ..Default::default()
            })
            .await
            .unwrap()
            .1;

        let mut media = crate::integration_test::seed_episode_with(
            &guard.0,
            db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt5550007".to_string()).ok(),
                ..Default::default()
            },
        )
        .await;
        let target =
            crate::services::media_tracker::resolve_target(&guard.0, &mut media)
                .await
                .unwrap()
                .expect("the series' imdb id alone already makes it matchable");
        assert_eq!(
            target
                .ids
                .tvdb,
            Some(303821),
            "the delivery path left the episode without ids of its own"
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/tok")
                .json_body(json!({
                    "Event": "MarkPlayed",
                    "Item": {
                        "Type": "Episode",
                        "Name": "The Target",
                        "SeriesName": "The Wire",
                        "ParentIndexNumber": 1,
                        "IndexNumber": 1,
                        "ProviderIds": {
                            "Tmdb": "972467",
                            "Imdb": "tt0749419",
                            "Tvdb": "303821",
                        },
                        "UserData": { "Played": true },
                    },
                }));
            then.status(200);
        });

        addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &target, &creds(), &ctx())
            .await
            .unwrap();

        mock.assert();
    }

    #[tokio::test]
    async fn a_start_reports_the_item_as_unplayed() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/tok")
                .json_body_partial(
                    r#"{ "Event": "Play", "Item": { "UserData": { "Played": false } } }"#,
                );
            then.status(200);
        });

        addon(&server)
            .on_event(
                &MediaTrackerEvent::PlaybackStart { position_ticks: 0 },
                &movie(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        mock.assert();
    }

    #[tokio::test]
    async fn a_rejected_token_asks_the_user_to_reconnect() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(401);
        });

        let err = addon(&server)
            .verify(&creds(), &ctx())
            .await
            .unwrap_err();

        assert!(!err.is_retryable());
        assert!(err.requires_reauth(), "the token is the thing to fix");
    }

    #[tokio::test]
    async fn an_outage_is_retried() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(503);
        });

        assert!(
            addon(&server)
                .verify(&creds(), &ctx())
                .await
                .unwrap_err()
                .is_retryable()
        );
    }

    #[tokio::test]
    async fn a_rate_limit_waits_as_long_as_it_was_told() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(429)
                .header("retry-after", "120");
        });

        let err = addon(&server)
            .verify(&creds(), &ctx())
            .await
            .unwrap_err();

        assert!(err.is_retryable());
        assert!(
            format!("{err:?}").contains("120"),
            "the header has to reach the backoff, got {err:?}"
        );
    }

    #[tokio::test]
    async fn connecting_checks_the_token_before_storing_it() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/fresh")
                .json_body(json!({}));
            then.status(200);
        });

        let stored = addon(&server)
            .connect_with_token(&json!({ "token": " fresh " }), &ctx())
            .await
            .unwrap();

        mock.assert();
        assert_eq!(stored.get_str("token"), Some("fresh"));
    }

    #[tokio::test]
    async fn connecting_with_a_bad_token_fails_on_the_form() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(401);
        });

        assert!(
            addon(&server)
                .connect_with_token(&json!({ "token": "nope" }), &ctx())
                .await
                .is_err()
        );
    }
}
