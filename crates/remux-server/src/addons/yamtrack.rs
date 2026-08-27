//! Yamtrack, reached through its Jellyfin webhook or, optionally, its API.
//!
//! The webhook is what released Yamtrack ships. It accepts four events and
//! reads nothing off an item but its external ids, its type, and a played
//! flag; an episode in particular is matched only on its own tvdb or imdb id.
//! Scores and favourites are things Yamtrack keeps and neither route can set,
//! which is why the capabilities claim neither.
//!
//! The API, unreleased upstream as of August 2026, addresses media in TMDB
//! terms instead: an episode is its show plus season and episode number,
//! which remux stores for every episode. The `use_api` option switches
//! delivery over for an instance built with it.

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
        jellyfin_webhook_body::{
            NotificationType, WebhookFormat, WebhookItem, classify, post, provider_ids,
            retry_after,
        },
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
            options: vec![
                AddonOption {
                    id: "base_url".to_string(),
                    name: "Yamtrack URL".to_string(),
                    description: Some(
                        "Where your Yamtrack lives, for example https://yamtrack.example.com."
                            .to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "use_api".to_string(),
                    name: "Use the API".to_string(),
                    description: Some(
                        "Deliver through Yamtrack's API instead of its Jellyfin \
                         webhook. The API matches an episode by its show plus \
                         season and episode number, so episodes without ids of \
                         their own still land. Needs a Yamtrack built with the \
                         API, which no release ships yet."
                            .to_string(),
                    ),
                    required: false,
                    default: None,
                    kind: AddonOptionType::Boolean,
                },
            ],
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
        let use_api = cfg
            .get("use_api")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(AddonCapabilities {
            media_tracker: Some(Arc::new(YamtrackAddon {
                base_url,
                client: super::make_http_client(),
                use_api,
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
    use_api: bool,
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

/// Yamtrack's api encodes a status as an integer; these two are the only ones
/// a playback event can mean.
const STATUS_IN_PROGRESS: i64 = 1;
const STATUS_COMPLETED: i64 = 3;

/// The API a Yamtrack built from its `feat/add-api` branch serves, which no
/// release ships as of August 2026. The same account token the webhook uses
/// authenticates it, as a bearer header instead of a path segment.
impl YamtrackAddon {
    fn api_url(&self, path: &str) -> String {
        format!("{}/api/v1/{}", self.base_url, path)
    }

    /// What a 404 from a route every api build serves means: an instance
    /// without the api, which answers page-not-found to every api call.
    fn without_api() -> MediaTrackerError {
        MediaTrackerError::permanent(
            "the tracker has no api at /api/v1: this Yamtrack does not ship \
             the api, so turn the addon's api option off",
        )
    }

    /// One api call, with refusals classified the way the webhook path
    /// classifies them. A success or a 404 comes back as a status for the
    /// caller to read, because a 404 is a real answer here: an untracked row
    /// on a detail route, or an instance without the api at all.
    async fn api_call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> MediaTrackerResult<reqwest::StatusCode> {
        let mut request = self
            .client
            .request(method, self.api_url(path))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| {
                // Same rule as the webhook path: the token travels in a header
                // here, but the url still names the user's instance, so the
                // error must not carry it.
                MediaTrackerError::retryable(format!(
                    "calling the tracker api: {}",
                    e.without_url()
                ))
            })?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(status);
        }
        Err(classify(status, retry_after(&response)))
    }

    /// The cheapest authenticated read the api has, shared by connect and
    /// verify: it proves the token and that the api exists at all.
    async fn api_probe(&self, token: &str) -> MediaTrackerResult<()> {
        let status = self
            .api_call(reqwest::Method::GET, "lists/", token, None)
            .await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::without_api());
        }
        Ok(())
    }

    async fn deliver_api(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        token: &str,
    ) -> MediaTrackerResult<()> {
        if matches!(
            event,
            MediaTrackerEvent::Favorite { .. } | MediaTrackerEvent::Rating { .. }
        ) {
            return Err(MediaTrackerError::unsupported(
                &event
                    .kind()
                    .to_string(),
            ));
        }
        if target.kind == db::MediaKind::Episode {
            self.deliver_api_episode(event, target, token)
                .await
        } else {
            self.deliver_api_movie(event, target, token)
                .await
        }
    }

    /// Whether the webhook could still match this item if the api cannot
    /// address it: an episode on its own tvdb or imdb id, anything else on
    /// any id at all.
    fn webhook_could_match(target: &MediaTrackerTarget) -> bool {
        if target.kind == db::MediaKind::Episode {
            target
                .ids
                .imdb
                .is_some()
                || target
                    .ids
                    .tvdb
                    .is_some()
        } else {
            provider_ids(&target.ids)
                .as_object()
                .is_some_and(|m| !m.is_empty())
        }
    }

    async fn deliver_api_episode(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        token: &str,
    ) -> MediaTrackerResult<()> {
        let coordinates = target
            .series
            .as_deref()
            .and_then(|series| {
                Some((
                    series
                        .ids
                        .tmdb?,
                    target.season?,
                    target.episode?,
                ))
            });
        let Some((show, season, episode)) = coordinates else {
            // Nothing to address it by in TMDB terms; the webhook may still
            // match it on its own ids.
            return self
                .deliver_webhook(event, target, token)
                .await;
        };
        match event {
            MediaTrackerEvent::PlaybackStop { played: true, .. }
            | MediaTrackerEvent::MarkPlayed => {
                let created = self
                    .api_call(
                        reqwest::Method::POST,
                        &format!("media/tv/tmdb/{show}/{season}/{episode}/history/"),
                        token,
                        Some(&json!({})),
                    )
                    .await?;
                if created != reqwest::StatusCode::NOT_FOUND {
                    return Ok(());
                }
                // Two things answer 404 here: an instance without the api,
                // and coordinates TMDB does not list, which the api refuses
                // to track. The probe settles which.
                self.api_probe(token)
                    .await?;
                // TMDB really does not know these numbers. Parts of a library
                // are numbered in another provider's scheme, TVDB's year
                // seasons for one, and for those the episode's own ids are
                // the identity that still works, so the webhook has the
                // better chance. Its refusal message names what was missing
                // if it cannot either.
                self.deliver_webhook(event, target, token)
                    .await
            }
            MediaTrackerEvent::MarkUnplayed => {
                let deleted = self
                    .api_call(
                        reqwest::Method::DELETE,
                        &format!("media/tv/tmdb/{show}/{season}/{episode}/"),
                        token,
                        None,
                    )
                    .await?;
                if deleted != reqwest::StatusCode::NOT_FOUND {
                    return Ok(());
                }
                // Nothing tracked under these coordinates. It may have been
                // tracked through the webhook under the coordinates Yamtrack
                // resolved for itself, so unmark that way too when the item
                // could match; a miss with no ids is already the state asked
                // for.
                if Self::webhook_could_match(target) {
                    return self
                        .deliver_webhook(event, target, token)
                        .await;
                }
                Ok(())
            }
            // A start or an abandoned stop: Yamtrack's own webhook records
            // nothing below the episode for these either, so there is no
            // call to make.
            _ => Ok(()),
        }
    }

    async fn deliver_api_movie(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        token: &str,
    ) -> MediaTrackerResult<()> {
        let Some(tmdb) = target
            .ids
            .tmdb
        else {
            // The api names a movie only by tmdb id; the webhook can still
            // match one on imdb.
            return self
                .deliver_webhook(event, target, token)
                .await;
        };
        let detail = format!("media/movie/tmdb/{tmdb}/");
        if matches!(event, MediaTrackerEvent::MarkUnplayed) {
            // Parity with the webhook, which deletes the tracked movie. A 404
            // means nothing was tracked, which is already the state asked for.
            self.api_call(reqwest::Method::DELETE, &detail, token, None)
                .await?;
            return Ok(());
        }
        // Everything else lands as a status. Yamtrack's webhook treats a
        // start and a stop identically, so this route does too.
        let status_code = match event {
            MediaTrackerEvent::PlaybackStop { played: true, .. }
            | MediaTrackerEvent::MarkPlayed => STATUS_COMPLETED,
            _ => STATUS_IN_PROGRESS,
        };
        let updated = self
            .api_call(
                reqwest::Method::PATCH,
                &detail,
                token,
                Some(&json!({ "status": status_code })),
            )
            .await?;
        if updated != reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        // Untracked, or an instance without the api; creating answers both.
        let body = json!({
            "source": "tmdb",
            "media_id": tmdb.to_string(),
            "status": status_code,
        });
        let created = self
            .api_call(reqwest::Method::POST, "media/movie/", token, Some(&body))
            .await?;
        if created == reqwest::StatusCode::NOT_FOUND {
            return Err(Self::without_api());
        }
        Ok(())
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

        if self.use_api {
            self.api_probe(token)
                .await?;
        } else {
            // An empty body carries no event, so Yamtrack answers the token
            // and nothing else. Anything with an `Event` would be a real
            // scrobble.
            post(&self.client, &self.webhook_url(token), &json!({})).await?;
        }
        Ok(MediaTrackerCredentials::new(json!({ "token": token })))
    }

    async fn verify(
        &self,
        creds: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let token = Self::token(creds)?;
        if self.use_api {
            return self
                .api_probe(token)
                .await;
        }
        post(&self.client, &self.webhook_url(token), &json!({})).await
    }

    async fn on_event(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        creds: &MediaTrackerCredentials,
        _ctx: &MediaTrackerCtx,
    ) -> MediaTrackerResult<()> {
        let token = Self::token(creds)?;
        if self.use_api {
            return self
                .deliver_api(event, target, token)
                .await;
        }
        self.deliver_webhook(event, target, token)
            .await
    }
}

impl YamtrackAddon {
    async fn deliver_webhook(
        &self,
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
        token: &str,
    ) -> MediaTrackerResult<()> {
        let item = WebhookItem::from_event(event, target).ok_or_else(|| {
            MediaTrackerError::unsupported(
                &event
                    .kind()
                    .to_string(),
            )
        })?;
        // Yamtrack reads the episode's own tvdb or imdb id and nothing else,
        // a tmdb id included: its TV path never looks at that field. So an
        // episode carrying only a tmdb id identifies nothing to it, even
        // though `provider_ids` has something to send. Posting anyway takes a
        // 200 and marks the row delivered, because `is_matchable` is satisfied
        // by the series' ids this provider never reads. Refusing here is what
        // puts the failure somewhere the user can see it.
        //
        // A movie is matched on any of the three, so only the episode rule is
        // narrower than "carries an id at all".
        let matchable = if item.kind == db::MediaKind::Episode {
            item.ids
                .imdb
                .is_some()
                || item
                    .ids
                    .tvdb
                    .is_some()
        } else {
            provider_ids(&item.ids)
                .as_object()
                .is_some_and(|m| !m.is_empty())
        };
        if !matchable {
            return Err(MediaTrackerError::permanent(
                if item.kind == db::MediaKind::Episode {
                    "no imdb or tvdb id to match the episode on"
                } else {
                    "no tmdb, imdb or tvdb id to match on"
                },
            ));
        }
        post(
            &self.client,
            &self.webhook_url(token),
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
            use_api: false,
        }
    }

    fn api_addon(server: &MockServer) -> YamtrackAddon {
        YamtrackAddon {
            use_api: true,
            ..addon(server)
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
            use_api: false,
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

    /// The shape a season-0 special arrives in: the series is identified, but
    /// TMDB carries no external ids for the episode, so completion leaves it
    /// with nothing of its own. `is_matchable` still passes it, on the series'
    /// ids that this provider never reads.
    #[tokio::test]
    async fn an_episode_with_no_ids_of_its_own_is_refused_rather_than_posted() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200);
        });

        let mut target = episode();
        target.ids = db::ExternalIds::default();

        let err = addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &target, &creds(), &ctx())
            .await
            .expect_err("nothing on the episode identifies it to Yamtrack");

        assert!(
            matches!(err, MediaTrackerError::Permanent { .. }),
            "a retry cannot conjure ids: {err:?}"
        );
        assert_eq!(
            mock.hits(),
            0,
            "the delivery went out and came back a 200 that recorded nothing"
        );
    }

    /// The shape most of a real library is in: TMDB gives an episode its own
    /// tmdb id and no `external_ids`. `provider_ids` has something to send, so
    /// an emptiness check passes it, but Yamtrack's TV path never reads tmdb.
    #[tokio::test]
    async fn an_episode_known_only_by_tmdb_is_refused() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200);
        });

        let mut target = episode();
        target.ids = db::ExternalIds {
            tmdb: Some(4762043),
            ..Default::default()
        };

        let err = addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &target, &creds(), &ctx())
            .await
            .expect_err("a tmdb id names an episode to nobody here");

        assert!(
            matches!(err, MediaTrackerError::Permanent { .. }),
            "{err:?}"
        );
        assert_eq!(
            mock.hits(),
            0,
            "the delivery went out carrying an id this provider does not read"
        );
    }

    /// The same id on a movie is enough, so the narrower rule is the episode's
    /// alone.
    #[tokio::test]
    async fn a_movie_known_only_by_tmdb_still_goes_out() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(200);
        });

        let mut target = movie();
        target.ids = db::ExternalIds {
            tmdb: Some(949),
            ..Default::default()
        };

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

    /// The case the webhook can never deliver: an episode with no external
    /// ids at all, under a series known only to TMDB.
    fn episode_known_only_by_coordinates() -> MediaTrackerTarget {
        MediaTrackerTarget {
            kind: db::MediaKind::Episode,
            title: "Episode 101".into(),
            year: None,
            ids: db::ExternalIds::default(),
            series: Some(Box::new(MediaTrackerTarget {
                kind: db::MediaKind::Series,
                title: "Blue's Clues".into(),
                year: Some(1996),
                ids: db::ExternalIds {
                    tmdb: Some(10821),
                    ..Default::default()
                },
                series: None,
                season: None,
                episode: None,
            })),
            season: Some(6),
            episode: Some(14),
        }
    }

    #[tokio::test]
    async fn the_api_tracks_an_episode_with_no_ids_of_its_own() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/media/tv/tmdb/10821/6/14/history/")
                .header("authorization", "Bearer tok")
                .json_body(json!({}));
            then.status(201);
        });

        api_addon(&server)
            .on_event(
                &MediaTrackerEvent::PlaybackStop {
                    position_ticks: 0,
                    played: true,
                },
                &episode_known_only_by_coordinates(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        mock.assert();
    }

    #[tokio::test]
    async fn an_episode_with_neither_coordinates_nor_ids_is_refused() {
        let mut target = episode_known_only_by_coordinates();
        target
            .series
            .as_mut()
            .unwrap()
            .ids
            .tmdb = None;

        // An unroutable address: reaching the network at all would come back
        // as a retryable transport error, not the permanent refusal expected.
        // With no coordinates the api route hands over to the webhook, whose
        // guard refuses before anything is sent.
        let addon = YamtrackAddon {
            base_url: "http://127.0.0.1:1".to_string(),
            client: reqwest::Client::new(),
            use_api: true,
        };
        let err = addon
            .on_event(&MediaTrackerEvent::MarkPlayed, &target, &creds(), &ctx())
            .await
            .expect_err("nothing to address the episode by");

        assert!(
            err.to_string()
                .contains("imdb"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn coordinates_tmdb_does_not_list_fall_back_to_the_webhook() {
        let mut target = episode_known_only_by_coordinates();
        target
            .ids
            .tvdb = Some(5711666);

        let server = MockServer::start();
        let history = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/media/tv/tmdb/10821/6/14/history/");
            then.status(404)
                .json_body(json!({ "detail": "Episode not found." }));
        });
        let probe = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/lists/");
            then.status(200)
                .json_body(json!([]));
        });
        let webhook = server.mock(|when, then| {
            when.method(POST)
                .path("/webhook/jellyfin/tok")
                .json_body_partial(
                    r#"{ "Item": { "ProviderIds": { "Tvdb": "5711666" } } }"#,
                );
            then.status(200);
        });

        api_addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &target, &creds(), &ctx())
            .await
            .unwrap();

        history.assert();
        probe.assert();
        webhook.assert();
    }

    #[tokio::test]
    async fn the_api_shrugs_when_unmarking_an_episode_it_never_tracked() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/api/v1/media/tv/tmdb/10821/6/14/");
            then.status(404);
        });

        api_addon(&server)
            .on_event(
                &MediaTrackerEvent::MarkUnplayed,
                &episode_known_only_by_coordinates(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        mock.assert();
    }

    #[tokio::test]
    async fn an_abandoned_episode_stop_makes_no_api_call() {
        let server = MockServer::start();
        let catch_all = server.mock(|when, then| {
            when.path_contains("/");
            then.status(500);
        });

        api_addon(&server)
            .on_event(
                &MediaTrackerEvent::PlaybackStop {
                    position_ticks: 5,
                    played: false,
                },
                &episode_known_only_by_coordinates(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        assert_eq!(catch_all.hits(), 0);
    }

    #[tokio::test]
    async fn the_api_updates_a_movie_already_tracked() {
        let server = MockServer::start();
        let patch = server.mock(|when, then| {
            when.method("PATCH")
                .path("/api/v1/media/movie/tmdb/949/")
                .json_body(json!({ "status": 3 }));
            then.status(200);
        });

        api_addon(&server)
            .on_event(&MediaTrackerEvent::MarkPlayed, &movie(), &creds(), &ctx())
            .await
            .unwrap();

        patch.assert();
    }

    #[tokio::test]
    async fn the_api_creates_a_movie_on_its_first_delivery() {
        let server = MockServer::start();
        let patch = server.mock(|when, then| {
            when.method("PATCH")
                .path("/api/v1/media/movie/tmdb/949/");
            then.status(404);
        });
        let create = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/media/movie/")
                .json_body(json!({
                    "source": "tmdb",
                    "media_id": "949",
                    "status": 1,
                }));
            then.status(201);
        });

        api_addon(&server)
            .on_event(
                &MediaTrackerEvent::PlaybackStop {
                    position_ticks: 5,
                    played: false,
                },
                &movie(),
                &creds(),
                &ctx(),
            )
            .await
            .unwrap();

        patch.assert();
        create.assert();
    }

    #[tokio::test]
    async fn a_yamtrack_without_the_api_is_told_apart_from_a_miss() {
        // Everything 404s on an instance that never routes /api/v1, so the
        // create call, which exists on every api build, is what settles it.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.path_contains("/");
            then.status(404);
        });

        let err = api_addon(&server)
            .on_event(
                &MediaTrackerEvent::MarkPlayed,
                &episode_known_only_by_coordinates(),
                &creds(),
                &ctx(),
            )
            .await
            .expect_err("a 404 from the create route means no api");

        assert!(
            err.to_string()
                .contains("api"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn connecting_in_api_mode_proves_the_token_against_the_api() {
        let server = MockServer::start();
        let probe = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/lists/")
                .header("authorization", "Bearer fresh");
            then.status(200)
                .json_body(json!([]));
        });

        let stored = api_addon(&server)
            .connect_with_token(&json!({ "token": " fresh " }), &ctx())
            .await
            .unwrap();

        probe.assert();
        assert_eq!(stored.get_str("token"), Some("fresh"));
    }
}
