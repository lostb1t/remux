//! Tracking capability: syncing a user's watch activity with an external
//! service (Trakt, Yamtrack). Unlike other capabilities this is per-user —
//! the operator configures the addon, each user connects it separately.

use crate::db;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

use super::AddonKind;
use async_trait::async_trait;

/// Split by what the dispatcher should do next, not by cause.
#[derive(Debug)]
pub enum TrackingError {
    /// Rate limited, 5xx, network. `retry_after` is a provider hint; the
    /// dispatcher waits the longer of it and its own backoff.
    Retryable {
        message: String,
        retry_after: Option<Duration>,
    },
    /// `reauth_required` drives the Reconnect prompt in the UI.
    Permanent {
        message: String,
        reauth_required: bool,
    },
}

impl TrackingError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn retry_after(message: impl Into<String>, after: Duration) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after: Some(after),
        }
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self::Permanent {
            message: message.into(),
            reauth_required: false,
        }
    }

    pub fn reauth(message: impl Into<String>) -> Self {
        Self::Permanent {
            message: message.into(),
            reauth_required: true,
        }
    }

    /// Backstop for a capability the provider never declared. Core gates on
    /// `TrackingCapabilities` first, so this firing means the two disagree.
    pub fn unsupported(what: &str) -> Self {
        Self::permanent(format!("provider does not support {what}"))
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable { .. })
    }

    pub fn requires_reauth(&self) -> bool {
        matches!(
            self,
            Self::Permanent {
                reauth_required: true,
                ..
            }
        )
    }
}

impl std::fmt::Display for TrackingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retryable {
                message,
                retry_after: Some(after),
            } => {
                write!(f, "{message} (retry after {}s)", after.as_secs())
            }
            Self::Retryable { message, .. } => write!(f, "{message}"),
            Self::Permanent {
                message,
                reauth_required: true,
            } => {
                write!(f, "{message} (reconnect required)")
            }
            Self::Permanent { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for TrackingError {}

pub type TrackingResult<T> = std::result::Result<T, TrackingError>;

/// Unit a per-user event filter is expressed in. Serialised into
/// `user_media_trackers.event_filters`, so renaming a variant is a migration.
#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TrackingEventKind {
    PlaybackStart,
    PlaybackProgress,
    PlaybackStop,
    MarkPlayed,
    MarkUnplayed,
    Favorite,
    Rating,
}

/// One thing that happened to one item, for one user.
#[derive(Debug, Clone)]
pub enum TrackingEvent {
    PlaybackStart {
        position_ticks: i64,
    },
    PlaybackProgress {
        position_ticks: i64,
        is_paused: bool,
    },
    PlaybackStop {
        position_ticks: i64,
        /// Passed the watched threshold. Providers scrobble a finish
        /// differently from an abandon.
        played: bool,
    },
    MarkPlayed,
    MarkUnplayed,
    Favorite {
        is_favorite: bool,
    },
    Rating {
        /// The user's own 0-10 rating, or `None` when they cleared it.
        /// Providers using a different scale rescale on the way out.
        rating: Option<f32>,
    },
}

impl TrackingEvent {
    pub fn kind(&self) -> TrackingEventKind {
        match self {
            Self::PlaybackStart { .. } => TrackingEventKind::PlaybackStart,
            Self::PlaybackProgress { .. } => TrackingEventKind::PlaybackProgress,
            Self::PlaybackStop { .. } => TrackingEventKind::PlaybackStop,
            Self::MarkPlayed => TrackingEventKind::MarkPlayed,
            Self::MarkUnplayed => TrackingEventKind::MarkUnplayed,
            Self::Favorite { .. } => TrackingEventKind::Favorite,
            Self::Rating { .. } => TrackingEventKind::Rating,
        }
    }

    pub fn position_ticks(&self) -> Option<i64> {
        match self {
            Self::PlaybackStart { position_ticks }
            | Self::PlaybackProgress { position_ticks, .. }
            | Self::PlaybackStop { position_ticks, .. } => Some(*position_ticks),
            Self::MarkPlayed
            | Self::MarkUnplayed
            | Self::Favorite { .. }
            | Self::Rating { .. } => None,
        }
    }
}

/// A media item resolved into what a provider needs to identify it remotely.
/// Core walks to the series via `Media::get_ancestors` once so addons never
/// need a DB handle. No `Default`: there is no meaningful default `MediaKind`.
#[derive(Debug, Clone)]
pub struct TrackingTarget {
    pub kind: db::MediaKind,
    pub title: String,
    pub year: Option<i32>,
    pub ids: TrackingIds,
    /// Set for episodes: the parent series' title, year and ids.
    pub series: Option<Box<TrackingTarget>>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

/// The ids tracking services key on — narrower than `db::ExternalIds`, which
/// also carries Deezer/Kitsu/IPTV/Stremio ids none of them understand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackingIds {
    pub imdb: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
}

impl TrackingIds {
    /// Nothing to match on. Core drops the action rather than queueing
    /// something no provider can act on.
    pub fn is_empty(&self) -> bool {
        self.imdb
            .is_none()
            && self
                .tmdb
                .is_none()
            && self
                .tvdb
                .is_none()
    }
}

/// Opaque to core: a static webhook token and an OAuth triple look the same.
pub type TrackingCredentials = remux_utils::Secret<serde_json::Value>;

/// Drives which connect UI the dashboard renders, without it knowing the
/// provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFlow {
    /// User pastes a value, described by `connect_fields`.
    Token,
    /// User enters a code on the provider's site; server polls. Suits TV
    /// clients with no browser.
    OAuthDeviceCode,
    /// Redirect plus callback. Needs a publicly reachable server.
    OAuthRedirect,
}

#[derive(Debug, Clone)]
pub struct DeviceAuthStart {
    pub verification_url: String,
    pub user_code: String,
    /// Opaque handle for `poll_device_auth`.
    pub poll_token: String,
    /// Providers reject polling faster than this.
    pub interval: Duration,
    pub expires_in: Duration,
}

#[derive(Debug, Clone)]
pub enum DeviceAuthPoll {
    Pending,
    Approved(TrackingCredentials),
    /// Declined or expired. Start over.
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncDirection {
    #[default]
    None,
    /// Remux to provider.
    Push,
    /// Provider to remux.
    Pull,
    Both,
}

impl SyncDirection {
    pub fn pushes(self) -> bool {
        matches!(self, Self::Push | Self::Both)
    }

    pub fn pulls(self) -> bool {
        matches!(self, Self::Pull | Self::Both)
    }
}

/// Static, no-I/O declaration of what a provider can do. Core reads these to
/// decide what to offer and what to call, so it never matches on provider id.
#[derive(Debug, Clone)]
pub struct TrackingCapabilities {
    pub auth_flow: AuthFlow,
    /// `Token` only. Reuses the addon option schema so the dashboard
    /// renders it with the same generic form code as addon settings.
    pub connect_fields: Vec<remux_sdks::remux::AddonOption>,
    pub supported_events: Vec<TrackingEventKind>,
    /// Must be a subset of `supported_events`.
    pub default_event_filter: Vec<TrackingEventKind>,
    pub history_import: bool,
    /// Partial playback positions, carried on `RemoteWatch::position_ticks` by
    /// `import_history` and `pull_changes` rather than by a method of its own.
    pub progress_import: bool,
    pub watch_state_sync: SyncDirection,
    pub favorites: SyncDirection,
    /// Personal 0-10 ratings. Separate from `favorites` because a provider can
    /// take one without the other.
    pub ratings: SyncDirection,
    pub watchlist: SyncDirection,
}

impl Default for TrackingCapabilities {
    /// Providers opt in, so a new capability never silently turns on for an
    /// existing addon.
    fn default() -> Self {
        Self {
            auth_flow: AuthFlow::Token,
            connect_fields: Vec::new(),
            supported_events: Vec::new(),
            default_event_filter: Vec::new(),
            history_import: false,
            progress_import: false,
            watch_state_sync: SyncDirection::None,
            favorites: SyncDirection::None,
            ratings: SyncDirection::None,
            watchlist: SyncDirection::None,
        }
    }
}

impl TrackingCapabilities {
    pub fn supports(&self, kind: TrackingEventKind) -> bool {
        self.supported_events
            .contains(&kind)
    }
}

/// Per-call context. No DB handle, matching `MetricsCtx`.
#[derive(Clone)]
pub struct TrackingCtx {
    pub config: Arc<crate::Config>,
}

/// One external tracking service. Bulk-sync methods default to `unsupported`
/// so a provider implements only what its capabilities advertise.
#[async_trait]
pub trait TrackingAddon: AddonKind + Send + Sync {
    /// Must be cheap and do no I/O — called while rendering pages.
    fn capabilities(&self) -> TrackingCapabilities;

    /// Should hit the provider so a bad token is rejected while the user is
    /// still on the form, not later as a failed scrobble.
    async fn connect_with_token(
        &self,
        _fields: &serde_json::Value,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<TrackingCredentials> {
        Err(TrackingError::unsupported("token authentication"))
    }

    async fn begin_device_auth(
        &self,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<DeviceAuthStart> {
        Err(TrackingError::unsupported("device-code authentication"))
    }

    async fn poll_device_auth(
        &self,
        _poll_token: &str,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<DeviceAuthPoll> {
        Err(TrackingError::unsupported("device-code authentication"))
    }

    async fn complete_redirect_auth(
        &self,
        _code: &str,
        _redirect_uri: &str,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<TrackingCredentials> {
        Err(TrackingError::unsupported("redirect authentication"))
    }

    /// Default suits providers whose credentials do not expire.
    async fn refresh(
        &self,
        creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<TrackingCredentials> {
        Ok(creds.clone())
    }

    /// Backs the connection health indicator. Should be cheap.
    async fn verify(
        &self,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<()> {
        Ok(())
    }

    /// Core deletes its row regardless: disconnecting must succeed locally
    /// even when the provider is down.
    async fn disconnect(
        &self,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<()> {
        Ok(())
    }

    async fn on_event(
        &self,
        event: &TrackingEvent,
        target: &TrackingTarget,
        creds: &TrackingCredentials,
        ctx: &TrackingCtx,
    ) -> TrackingResult<()>;

    async fn import_history(
        &self,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<Vec<RemoteWatch>> {
        Err(TrackingError::unsupported("history import"))
    }

    /// `None` for a full sweep.
    async fn pull_changes(
        &self,
        _since: Option<chrono::NaiveDateTime>,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<Vec<RemoteWatch>> {
        Err(TrackingError::unsupported("external change sync"))
    }

    async fn pull_watchlist(
        &self,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<Vec<TrackingIds>> {
        Err(TrackingError::unsupported("watchlist sync"))
    }

    async fn push_watchlist(
        &self,
        _target: &TrackingTarget,
        _add: bool,
        _creds: &TrackingCredentials,
        _ctx: &TrackingCtx,
    ) -> TrackingResult<()> {
        Err(TrackingError::unsupported("watchlist sync"))
    }
}

/// One item's user data read back from a provider, by `import_history` and
/// `pull_changes`.
#[derive(Debug, Clone)]
pub struct RemoteWatch {
    pub ids: TrackingIds,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub watched: bool,
    /// Present only when the provider reports partial progress.
    pub position_ticks: Option<i64>,
    pub watched_at: Option<chrono::NaiveDateTime>,
    /// `None` when the provider does not report favourites. Without this a
    /// provider could declare `favorites: Pull` that core had no way to act on.
    pub favorite: Option<bool>,
    /// The remote 0-10 rating, `None` when the provider does not report one.
    /// Same reasoning as `favorite`: `ratings: Pull` needs somewhere to land.
    pub rating: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_and_permanent_are_distinguishable() {
        assert!(TrackingError::retryable("boom").is_retryable());
        assert!(!TrackingError::permanent("nope").is_retryable());
        assert!(!TrackingError::reauth("bad token").is_retryable());
    }

    #[test]
    fn only_reauth_errors_ask_the_user_to_reconnect() {
        assert!(TrackingError::reauth("401").requires_reauth());
        assert!(!TrackingError::permanent("400 bad request").requires_reauth());
        assert!(!TrackingError::retryable("timeout").requires_reauth());
    }

    #[test]
    fn unsupported_is_permanent_and_never_retried() {
        let err = TrackingError::unsupported("watchlist sync");
        assert!(!err.is_retryable());
        assert!(!err.requires_reauth());
        assert!(
            err.to_string()
                .contains("watchlist sync")
        );
    }

    #[test]
    fn retry_after_is_carried_and_shown() {
        let err = TrackingError::retry_after("429", Duration::from_secs(30));
        match &err {
            TrackingError::Retryable {
                retry_after: Some(d),
                ..
            } => assert_eq!(d.as_secs(), 30),
            other => panic!("expected a retry-after hint, got {other:?}"),
        }
        assert!(
            err.to_string()
                .contains("30")
        );
    }

    /// Filters are stored as strings; a variant that does not round-trip would
    /// silently drop a user's filter on reload.
    #[test]
    fn event_kinds_round_trip_through_their_string_form() {
        for kind in [
            TrackingEventKind::PlaybackStart,
            TrackingEventKind::PlaybackProgress,
            TrackingEventKind::PlaybackStop,
            TrackingEventKind::MarkPlayed,
            TrackingEventKind::MarkUnplayed,
            TrackingEventKind::Favorite,
            TrackingEventKind::Rating,
        ] {
            let s = kind.to_string();
            assert_eq!(
                s.parse::<TrackingEventKind>()
                    .unwrap(),
                kind
            );
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(
                serde_json::from_str::<TrackingEventKind>(&json).unwrap(),
                kind
            );
            // serde and strum must agree, or a filter written by the API would
            // not match one read by the dispatcher.
            assert_eq!(json, format!("\"{s}\""));
        }
    }

    #[test]
    fn every_event_reports_its_own_kind() {
        let cases = [
            (
                TrackingEvent::PlaybackStart { position_ticks: 0 },
                TrackingEventKind::PlaybackStart,
            ),
            (
                TrackingEvent::PlaybackProgress {
                    position_ticks: 1,
                    is_paused: true,
                },
                TrackingEventKind::PlaybackProgress,
            ),
            (
                TrackingEvent::PlaybackStop {
                    position_ticks: 2,
                    played: true,
                },
                TrackingEventKind::PlaybackStop,
            ),
            (TrackingEvent::MarkPlayed, TrackingEventKind::MarkPlayed),
            (TrackingEvent::MarkUnplayed, TrackingEventKind::MarkUnplayed),
            (
                TrackingEvent::Favorite { is_favorite: true },
                TrackingEventKind::Favorite,
            ),
            (
                TrackingEvent::Rating { rating: Some(7.0) },
                TrackingEventKind::Rating,
            ),
            (
                TrackingEvent::Rating { rating: None },
                TrackingEventKind::Rating,
            ),
        ];
        for (event, want) in cases {
            assert_eq!(event.kind(), want, "wrong kind for {event:?}");
        }
    }

    #[test]
    fn only_playback_events_carry_a_position() {
        assert_eq!(
            TrackingEvent::PlaybackStop {
                position_ticks: 99,
                played: false,
            }
            .position_ticks(),
            Some(99)
        );
        assert_eq!(TrackingEvent::MarkPlayed.position_ticks(), None);
        assert_eq!(
            TrackingEvent::Favorite { is_favorite: false }.position_ticks(),
            None
        );
        assert_eq!(
            TrackingEvent::Rating { rating: Some(7.0) }.position_ticks(),
            None
        );
    }

    #[test]
    fn ids_are_empty_only_when_nothing_is_matchable() {
        assert!(TrackingIds::default().is_empty());
        assert!(
            !TrackingIds {
                imdb: Some("tt123".into()),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !TrackingIds {
                tmdb: Some(603),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !TrackingIds {
                tvdb: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn sync_direction_resolves_both_ways() {
        assert!(!SyncDirection::None.pushes() && !SyncDirection::None.pulls());
        assert!(SyncDirection::Push.pushes() && !SyncDirection::Push.pulls());
        assert!(!SyncDirection::Pull.pushes() && SyncDirection::Pull.pulls());
        assert!(SyncDirection::Both.pushes() && SyncDirection::Both.pulls());
    }

    /// A provider that forgets to declare an event should send nothing.
    #[test]
    fn default_capabilities_grant_nothing() {
        let caps = TrackingCapabilities::default();
        assert!(
            caps.supported_events
                .is_empty()
        );
        assert!(!caps.history_import);
        assert!(!caps.progress_import);
        assert_eq!(caps.watch_state_sync, SyncDirection::None);
        assert_eq!(caps.favorites, SyncDirection::None);
        assert_eq!(caps.ratings, SyncDirection::None);
        assert_eq!(caps.watchlist, SyncDirection::None);
        assert!(!caps.supports(TrackingEventKind::PlaybackStop));
    }

    /// A provider declaring `favorites: Pull` needs somewhere to put one, and a
    /// favourite is independent of watched state.
    #[test]
    fn remote_user_data_can_carry_a_favourite_on_its_own() {
        let watch = RemoteWatch {
            ids: TrackingIds {
                tmdb: Some(603),
                ..Default::default()
            },
            season: None,
            episode: None,
            watched: false,
            position_ticks: None,
            watched_at: None,
            favorite: Some(true),
            rating: None,
        };
        assert_eq!(watch.favorite, Some(true));
        assert!(!watch.watched);
    }

    /// Same reasoning for `ratings: Pull`: a rating is independent of both
    /// watched state and favourite, so it has to survive on its own.
    #[test]
    fn remote_user_data_can_carry_a_rating_on_its_own() {
        let watch = RemoteWatch {
            ids: TrackingIds {
                tmdb: Some(603),
                ..Default::default()
            },
            season: None,
            episode: None,
            watched: false,
            position_ticks: None,
            watched_at: None,
            favorite: None,
            rating: Some(7.0),
        };
        assert_eq!(watch.rating, Some(7.0));
        assert_eq!(watch.favorite, None);
        assert!(!watch.watched);
    }
}
