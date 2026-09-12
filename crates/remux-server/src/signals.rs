use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use remux_sdks::remux::PlayMethod;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct PlaybackContext {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub played: bool,
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
    pub remote_endpoint: Option<String>,
    pub media_source_id: Option<String>,
    pub play_method: Option<PlayMethod>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

impl PlaybackContext {
    pub fn from_parts(
        session: &crate::db::auth::AuthSession,
        data: &remux_sdks::remux::PlaybackInfo,
        playback: Option<&crate::playback_session::PlaybackSession>,
        play_session_id: Option<&str>,
    ) -> Self {
        Self {
            session_id: play_session_id
                .map(str::to_owned)
                .or_else(|| {
                    data.play_session_id
                        .clone()
                })
                .or_else(|| {
                    playback.map(|p| {
                        p.play_session_id
                            .clone()
                    })
                })
                .unwrap_or_default(),
            device_id: session
                .device
                .id
                .clone(),
            device_name: session
                .device
                .name
                .clone(),
            client_name: session
                .device
                .app_name
                .clone(),
            remote_endpoint: session
                .device
                .remote_ip
                .clone(),
            media_source_id: data
                .media_source_id
                .clone()
                .or_else(|| {
                    playback.and_then(|p| {
                        p.media_source_id
                            .clone()
                    })
                }),
            play_method: data
                .play_method
                .clone()
                .or_else(|| {
                    playback
                        .and_then(|p| {
                            p.play_method
                                .as_deref()
                        })
                        .and_then(|m| {
                            m.parse()
                                .ok()
                        })
                }),
            audio_stream_index: data
                .audio_stream_index
                .or_else(|| playback.and_then(|p| p.audio_stream_index)),
            subtitle_stream_index: data
                .subtitle_stream_index
                .or_else(|| playback.and_then(|p| p.subtitle_stream_index)),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MarkPlayedInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct MarkUnplayedInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct MarkFavoriteInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct UnmarkFavoriteInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
}

#[derive(Debug, Clone, Default)]
pub struct RatingInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub rating: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct UserUpdatedInfo {
    pub user_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct UserDeletedInfo {
    pub user_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct RemotePlayInfo {
    pub device_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RemotePlaystateInfo {
    pub device_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RemoteCommandInfo {
    pub device_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum Event {
    PlaybackStarted(PlaybackContext),
    PlaybackProgress(PlaybackContext),
    PlaybackStopped(PlaybackContext),
    MarkPlayed(MarkPlayedInfo),
    MarkUnplayed(MarkUnplayedInfo),
    MarkFavorite(MarkFavoriteInfo),
    UnmarkFavorite(UnmarkFavoriteInfo),
    Rating(RatingInfo),
    UserUpdated(UserUpdatedInfo),
    UserDeleted(UserDeletedInfo),
    LibraryChanged,
    SessionsChanged,
    RemotePlay(RemotePlayInfo),
    RemotePlaystate(RemotePlaystateInfo),
    RemoteCommand(RemoteCommandInfo),
    /// A playback session ended — explicit stop, or silently evicted by a
    /// new session on the same device — and whatever source it was using is
    /// no longer needed. Fired for both transcode and direct-play sessions
    /// alike (there's no `TranscodeSession` at all for direct play), so
    /// subscribers re-derive the actual stream source the same way the
    /// original request did rather than relying on it being captured
    /// up front. Carries enough of the session to redo that lookup.
    PlaybackSessionEnded {
        item_id: Uuid,
        media_source_id: Option<String>,
        device_id: String,
        user_id: Uuid,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    PlaybackStarted,
    PlaybackProgress,
    PlaybackStopped,
    MarkPlayed,
    MarkUnplayed,
    MarkFavorite,
    UnmarkFavorite,
    Rating,
    UserUpdated,
    UserDeleted,
    LibraryChanged,
    SessionsChanged,
    RemotePlay,
    RemotePlaystate,
    RemoteCommand,
    PlaybackSessionEnded,
}

impl Event {
    pub fn event_type(&self) -> EventType {
        match self {
            Event::PlaybackStarted(_) => EventType::PlaybackStarted,
            Event::PlaybackProgress(_) => EventType::PlaybackProgress,
            Event::PlaybackStopped(_) => EventType::PlaybackStopped,
            Event::MarkPlayed(_) => EventType::MarkPlayed,
            Event::MarkUnplayed(_) => EventType::MarkUnplayed,
            Event::MarkFavorite(_) => EventType::MarkFavorite,
            Event::UnmarkFavorite(_) => EventType::UnmarkFavorite,
            Event::Rating(_) => EventType::Rating,
            Event::UserUpdated(_) => EventType::UserUpdated,
            Event::UserDeleted(_) => EventType::UserDeleted,
            Event::LibraryChanged => EventType::LibraryChanged,
            Event::SessionsChanged => EventType::SessionsChanged,
            Event::RemotePlay(_) => EventType::RemotePlay,
            Event::RemotePlaystate(_) => EventType::RemotePlaystate,
            Event::RemoteCommand(_) => EventType::RemoteCommand,
            Event::PlaybackSessionEnded { .. } => EventType::PlaybackSessionEnded,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeliveryMode {
    Transient,
    Persistent { max_retries: Option<u32> },
}

#[async_trait]
pub trait Subscriber: Send + Sync {
    fn key(&self) -> &'static str;
    fn events(&self) -> &[EventType];
    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::Transient
    }
    async fn handle(&self, event: Event) -> anyhow::Result<()>;
}

#[derive(Clone, Default)]
pub struct Signals {
    subscribers: Vec<Arc<dyn Subscriber>>,
}

impl Signals {
    pub fn register(&mut self, s: impl Subscriber + 'static) {
        self.subscribers
            .push(Arc::new(s));
    }

    pub fn emit(&self, event: Event) {
        let kind = event.event_type();
        for sub in &self.subscribers {
            if !sub
                .events()
                .contains(&kind)
            {
                continue;
            }
            let (sub, event) = (sub.clone(), event.clone());
            match sub.delivery_mode() {
                DeliveryMode::Transient => {
                    tokio::spawn(async move {
                        if let Err(e) = sub
                            .handle(event)
                            .await
                        {
                            warn!(key = sub.key(), error = %e, "subscriber error");
                        }
                    });
                }
                DeliveryMode::Persistent { max_retries } => {
                    tokio::spawn(async move {
                        let mut attempt = 0u32;
                        loop {
                            match sub
                                .handle(event.clone())
                                .await
                            {
                                Ok(_) => break,
                                Err(e) => {
                                    attempt += 1;
                                    if max_retries.is_some_and(|m| attempt >= m) {
                                        warn!(
                                            key = sub.key(),
                                            error = %e,
                                            attempt,
                                            "subscriber exhausted retries"
                                        );
                                        break;
                                    }
                                    let delay = backoff_seconds(attempt);
                                    warn!(
                                        key = sub.key(),
                                        error = %e,
                                        attempt,
                                        delay_secs = delay,
                                        "subscriber error, retrying"
                                    );
                                    tokio::time::sleep(Duration::from_secs(delay))
                                        .await;
                                }
                            }
                        }
                    });
                }
            }
        }
    }
}

fn backoff_seconds(attempt: u32) -> u64 {
    (30 * 2u64.pow(
        attempt
            .saturating_sub(1)
            .min(10),
    ))
    .min(3600)
}
