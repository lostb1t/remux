use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info};
use uuid::Uuid;

use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSession {
    pub play_session_id: String,
    pub user_id: Uuid,
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub device_id: String,
    pub client_name: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume_level: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub play_method: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

const SESSION_TTL: Duration = Duration::from_secs(60 * 30); // 30 minutes
const SESSION_PREFIX: &str = "playback_session:";

fn session_key(play_session_id: &str) -> String {
    format!("{}{}", SESSION_PREFIX, play_session_id)
}

impl PlaybackSession {
    pub fn save(&self, store: &Store) {
        store.save(
            session_key(&self.play_session_id),
            self.clone(),
            SESSION_TTL,
        );
    }

    pub fn get(store: &Store, play_session_id: &str) -> Option<Self> {
        store.get::<Self>(session_key(play_session_id))
    }

    pub fn remove(store: &Store, play_session_id: &str) -> Option<Self> {
        let session = Self::get(store, play_session_id);
        store.delete(session_key(play_session_id));
        session
    }

    pub fn ping(store: &Store, play_session_id: &str) {
        if let Some(mut session) = Self::get(store, play_session_id) {
            session.last_activity = Utc::now();
            session.save(store);
            debug!("Pinged session: {}", play_session_id);
        }
    }

    /// Get all active playback sessions
    pub fn get_all(store: &Store) -> Vec<Self> {
        store
            .scan_keys(SESSION_PREFIX)
            .into_iter()
            .filter_map(|key| {
                let session_id = key.trim_start_matches(SESSION_PREFIX);
                Self::get(store, &session_id)
            })
            .collect()
    }
}
