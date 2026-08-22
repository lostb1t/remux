//! The half of a webhook-fed media tracker that all of them share.
//!
//! Yamtrack, Scrob, Ryot and Floppy are each fed through Jellyfin's webhook
//! plugin, but every one of them supplies its own Handlebars template, so the
//! JSON differs per tracker while the notification behind it does not. This
//! builds that notification and posts it; a [`WebhookFormat`] renders the
//! shape one tracker expects.

use std::time::Duration;

use crate::{
    addons::media_tracker::{
        MediaTrackerError, MediaTrackerEvent, MediaTrackerResult, MediaTrackerTarget,
    },
    db,
};

/// The plugin's notification kinds, limited to the ones a tracker acts on.
#[derive(
    strum_macros::EnumString, strum_macros::Display, Debug, Clone, Copy, PartialEq, Eq,
)]
pub enum NotificationType {
    PlaybackStart,
    PlaybackProgress,
    PlaybackStop,
    /// Jellyfin's kind for a watch state changed outside playback.
    UserDataSaved,
}

/// One notification, in the terms a template renders from. Named after the
/// plugin's own fields so a tracker's published template can be read against
/// this line by line.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookItem {
    pub notification_type: NotificationType,
    pub kind: db::MediaKind,
    pub name: String,
    pub year: Option<i32>,
    pub ids: db::ExternalIds,
    /// Set for episodes. Which of these a tracker keys on differs: Scrob
    /// matches the series plus the numbers below, Yamtrack matches the
    /// episode's own ids, so both are carried and the format picks.
    pub series_name: Option<String>,
    pub series_ids: Option<db::ExternalIds>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub played_to_completion: bool,
    pub position_ticks: Option<i64>,
}

impl WebhookItem {
    /// `None` when the event has no notification a template could carry.
    /// Favourites and ratings reach Jellyfin as `UserDataSaved` too, but no
    /// published template reads them, so they stay unsupported until one does.
    pub fn from_event(
        event: &MediaTrackerEvent,
        target: &MediaTrackerTarget,
    ) -> Option<Self> {
        let (notification_type, played, position_ticks) = match event {
            MediaTrackerEvent::PlaybackStart { position_ticks } => (
                NotificationType::PlaybackStart,
                false,
                Some(*position_ticks),
            ),
            MediaTrackerEvent::PlaybackProgress { position_ticks, .. } => (
                NotificationType::PlaybackProgress,
                false,
                Some(*position_ticks),
            ),
            MediaTrackerEvent::PlaybackStop {
                position_ticks,
                played,
            } => (
                NotificationType::PlaybackStop,
                *played,
                Some(*position_ticks),
            ),
            MediaTrackerEvent::MarkPlayed => {
                (NotificationType::UserDataSaved, true, None)
            }
            MediaTrackerEvent::MarkUnplayed => {
                (NotificationType::UserDataSaved, false, None)
            }
            MediaTrackerEvent::Favorite { .. } | MediaTrackerEvent::Rating { .. } => {
                return None;
            }
        };

        let series = target
            .series
            .as_deref();
        Some(Self {
            notification_type,
            kind: target
                .kind
                .clone(),
            name: target
                .title
                .clone(),
            year: target.year,
            ids: target
                .ids
                .clone(),
            series_name: series.map(|s| {
                s.title
                    .clone()
            }),
            series_ids: series.map(|s| {
                s.ids
                    .clone()
            }),
            season: target.season,
            episode: target.episode,
            played_to_completion: played,
            position_ticks,
        })
    }

    /// The series' ids for an episode, its own for anything else. Correct for
    /// a tracker that keys an episode on its show; one that keys on the
    /// episode itself should read [`WebhookItem::ids`] instead.
    pub fn matching_ids(&self) -> &db::ExternalIds {
        self.series_ids
            .as_ref()
            .unwrap_or(&self.ids)
    }
}

/// Renders one tracker's expected body. Everything else here is shared, so a
/// further tracker is an impl and a preset rather than another addon.
pub trait WebhookFormat: Send + Sync {
    fn body(&self, item: &WebhookItem) -> serde_json::Value;
}

/// Jellyfin sends every provider id as a string, and every webhook tracker
/// reads them straight out of the map, so numeric ids are stringified here.
/// Everything else `ExternalIds` carries identifies nothing to a webhook
/// tracker.
pub fn provider_ids(ids: &db::ExternalIds) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(tmdb) = ids.tmdb {
        map.insert("Tmdb".into(), serde_json::json!(tmdb.to_string()));
    }
    if let Some(imdb) = &ids.imdb {
        map.insert("Imdb".into(), serde_json::json!(imdb));
    }
    if let Some(tvdb) = ids.tvdb {
        map.insert("Tvdb".into(), serde_json::json!(tvdb.to_string()));
    }
    serde_json::Value::Object(map)
}

/// Posts `body`, translating the response into the dispatcher's terms.
pub async fn post(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> MediaTrackerResult<()> {
    let response = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| {
            // A tracker may carry its credential in the url, as a path segment
            // or a query parameter, and a reqwest error prints the url it
            // failed on. That string becomes `last_error` and the API hands it
            // back, so the url is dropped before the message is built.
            MediaTrackerError::retryable(format!(
                "posting to the tracker: {}",
                e.without_url()
            ))
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(classify(status, retry_after(&response)))
}

/// Only the delay-seconds form. The HTTP-date form exists, but no tracker here
/// sends it, and guessing wrong is worse than falling back to our own backoff.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// 401 and 403 are the user's credential and only they should cost the
/// connection. 408, 429 and 5xx pass on their own. Every other 4xx is this
/// request, which retrying will not change.
fn classify(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
) -> MediaTrackerError {
    match status.as_u16() {
        401 | 403 => MediaTrackerError::reauth(format!(
            "tracker rejected the credential ({status})"
        )),
        408 | 429 => match retry_after {
            Some(after) => MediaTrackerError::retry_after(
                format!("tracker is busy ({status})"),
                after,
            ),
            None => MediaTrackerError::retryable(format!("tracker is busy ({status})")),
        },
        _ if status.is_server_error() => {
            MediaTrackerError::retryable(format!("tracker failed ({status})"))
        }
        _ => MediaTrackerError::permanent(format!(
            "tracker refused the delivery ({status})"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(imdb: &str, tvdb: Option<i64>) -> db::ExternalIds {
        db::ExternalIds {
            imdb: db::NonEmptyString::try_new(imdb.to_string()).ok(),
            tvdb,
            ..Default::default()
        }
    }

    fn movie() -> MediaTrackerTarget {
        MediaTrackerTarget {
            kind: db::MediaKind::Movie,
            title: "Heat".into(),
            year: Some(1995),
            ids: ids("tt0113277", None),
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
            ids: db::ExternalIds::default(),
            series: Some(Box::new(MediaTrackerTarget {
                kind: db::MediaKind::Series,
                title: "The Wire".into(),
                year: Some(2002),
                ids: ids("tt0306414", Some(79126)),
                series: None,
                season: None,
                episode: None,
            })),
            season: Some(1),
            episode: Some(1),
        }
    }

    fn stop(played: bool) -> MediaTrackerEvent {
        MediaTrackerEvent::PlaybackStop {
            position_ticks: 42,
            played,
        }
    }

    #[test]
    fn an_episode_is_matched_on_its_series() {
        let item = WebhookItem::from_event(&stop(true), &episode()).unwrap();

        assert_eq!(item.series_name, Some("The Wire".to_string()));
        assert_eq!(item.season, Some(1));
        assert_eq!(item.episode, Some(1));
        assert_eq!(
            item.matching_ids()
                .tvdb,
            Some(79126)
        );
    }

    #[test]
    fn anything_else_is_matched_on_its_own_ids() {
        let item = WebhookItem::from_event(&stop(true), &movie()).unwrap();

        assert!(
            item.series_ids
                .is_none()
        );
        assert_eq!(
            item.matching_ids()
                .imdb
                .as_deref()
                .map(|s| s.to_string()),
            Some("tt0113277".to_string())
        );
    }

    #[test]
    fn a_stop_carries_whether_it_counted_as_a_watch() {
        assert!(
            WebhookItem::from_event(&stop(true), &movie())
                .unwrap()
                .played_to_completion
        );
        assert!(
            !WebhookItem::from_event(&stop(false), &movie())
                .unwrap()
                .played_to_completion
        );
    }

    #[test]
    fn marking_played_is_not_a_playback_notification() {
        let item =
            WebhookItem::from_event(&MediaTrackerEvent::MarkPlayed, &movie()).unwrap();

        assert_eq!(item.notification_type, NotificationType::UserDataSaved);
        assert!(item.played_to_completion);
        assert_eq!(
            item.position_ticks, None,
            "there was no playback to be positioned in"
        );
    }

    #[test]
    fn an_event_no_template_carries_has_no_notification() {
        for event in [
            MediaTrackerEvent::Favorite { is_favorite: true },
            MediaTrackerEvent::Rating { rating: Some(8.0) },
        ] {
            assert!(WebhookItem::from_event(&event, &movie()).is_none());
        }
    }

    async fn post_to(status: u16, retry_after: Option<&str>) -> MediaTrackerResult<()> {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::POST);
            let then = then.status(status);
            match retry_after {
                Some(v) => then.header("Retry-After", v),
                None => then,
            };
        });
        post(
            &reqwest::Client::new(),
            &server.url("/hook"),
            &serde_json::json!({}),
        )
        .await
    }

    #[tokio::test]
    async fn a_delivery_the_tracker_took_is_done() {
        assert!(
            post_to(204, None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_rejected_credential_asks_the_user_to_reconnect() {
        for status in [401, 403] {
            let err = post_to(status, None)
                .await
                .unwrap_err();
            assert!(err.requires_reauth(), "{status} should prompt a reconnect");
        }
    }

    #[tokio::test]
    async fn a_busy_tracker_is_retried_no_sooner_than_it_asked() {
        let err = post_to(429, Some("30"))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            MediaTrackerError::Retryable {
                retry_after: Some(after),
                ..
            } if after == Duration::from_secs(30)
        ));
    }

    #[tokio::test]
    async fn a_tracker_that_is_down_is_retried_on_our_own_backoff() {
        let err = post_to(503, None)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            MediaTrackerError::Retryable {
                retry_after: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_refused_delivery_is_not_worth_repeating() {
        let err = post_to(400, None)
            .await
            .unwrap_err();

        assert!(!err.is_retryable());
        assert!(
            !err.requires_reauth(),
            "the request was wrong, not the credential"
        );
    }
}
