use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PlaybackStartedInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct PlaybackProgressInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub session_id: String,
    pub position_ticks: i64,
    pub is_paused: bool,
}

#[derive(Debug, Clone)]
pub struct PlaybackStoppedInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub session_id: String,
    pub position_ticks: i64,
    pub played: bool,
}

#[derive(Debug, Clone)]
pub struct UserDataChangedInfo {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub kind: UserDataChangedKind,
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
pub enum UserDataChangedKind {
    Progress { position_ticks: i64 },
    Played,
    Unplayed,
    Favorite { is_favorite: bool },
    Rating { rating: Option<f32> },
}

#[derive(Debug, Clone)]
pub enum Event {
    PlaybackStarted(PlaybackStartedInfo),
    PlaybackProgress(PlaybackProgressInfo),
    PlaybackStopped(PlaybackStoppedInfo),
    UserDataChanged(UserDataChangedInfo),
    UserUpdated(UserUpdatedInfo),
    UserDeleted(UserDeletedInfo),
    LibraryChanged,
    SessionsChanged,
    RemotePlay(RemotePlayInfo),
    RemotePlaystate(RemotePlaystateInfo),
    RemoteCommand(RemoteCommandInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    PlaybackStarted,
    PlaybackProgress,
    PlaybackStopped,
    UserDataChanged,
    UserUpdated,
    UserDeleted,
    LibraryChanged,
    SessionsChanged,
    RemotePlay,
    RemotePlaystate,
    RemoteCommand,
}

impl Event {
    pub fn event_type(&self) -> EventType {
        match self {
            Event::PlaybackStarted(_) => EventType::PlaybackStarted,
            Event::PlaybackProgress(_) => EventType::PlaybackProgress,
            Event::PlaybackStopped(_) => EventType::PlaybackStopped,
            Event::UserDataChanged(_) => EventType::UserDataChanged,
            Event::UserUpdated(_) => EventType::UserUpdated,
            Event::UserDeleted(_) => EventType::UserDeleted,
            Event::LibraryChanged => EventType::LibraryChanged,
            Event::SessionsChanged => EventType::SessionsChanged,
            Event::RemotePlay(_) => EventType::RemotePlay,
            Event::RemotePlaystate(_) => EventType::RemotePlaystate,
            Event::RemoteCommand(_) => EventType::RemoteCommand,
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
    (30 * 2u64.pow(attempt.saturating_sub(1))).min(1800)
}
