use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::info;
use uuid::Uuid;

use crate::transcode::session::TranscodeSession;
use remux_sdks::remux::QueueItem;

#[derive(Clone)]
pub struct PlaybackSession {
    pub play_session_id: String,
    pub user_id: Uuid,
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub device_id: String,
    pub client_name: String,
    pub position_ticks: i64,
    pub can_seek: bool,
    pub is_paused: bool,
    pub last_paused_at: Option<DateTime<Utc>>,
    pub is_muted: bool,
    pub volume_level: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub play_method: Option<String>,
    pub now_playing_queue: Option<Vec<QueueItem>>,
    pub playlist_item_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    /// Active transcode session owned by this playback session, if any.
    pub transcode: Option<Arc<tokio::sync::RwLock<TranscodeSession>>>,
}

#[derive(Clone)]
pub struct PlaybackSessionManager {
    sessions: Arc<DashMap<String, PlaybackSession>>,
    base_dir: PathBuf,
}

impl PlaybackSessionManager {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let _ = std::fs::create_dir_all(&base_dir);
        Self {
            sessions: Arc::new(DashMap::new()),
            base_dir,
        }
    }

    /// Insert (or replace) a playback session, preserving any transcode that was
    /// pre-attached before `report_playback_start` fired.
    /// Removes stale sessions for the same device so `get_sessions` always
    /// finds the most recent playback.
    pub fn insert(&self, mut session: PlaybackSession) {
        if session.transcode.is_none() {
            if let Some(existing) = self.sessions.get(&session.play_session_id) {
                session.transcode = existing.value().transcode.clone();
            }
        }
        // Remove any previous session for this device (different play_session_id).
        if !session.device_id.is_empty() {
            let stale: Vec<String> = self
                .sessions
                .iter()
                .filter(|e| {
                    e.value().device_id == session.device_id
                        && e.key() != &session.play_session_id
                })
                .map(|e| e.key().clone())
                .collect();
            for id in stale {
                self.sessions.remove(&id);
            }
        }
        self.sessions
            .insert(session.play_session_id.clone(), session);
    }

    /// Return a clone of the session, if it exists.
    pub fn get(&self, id: &str) -> Option<PlaybackSession> {
        self.sessions.get(id).map(|e| e.value().clone())
    }

    /// Return a clone of the transcode session attached to this playback session.
    pub fn get_transcode(
        &self,
        id: &str,
    ) -> Option<Arc<tokio::sync::RwLock<TranscodeSession>>> {
        self.sessions.get(id)?.value().transcode.clone()
    }

    /// Return clones of all active sessions.
    pub fn get_all(&self) -> Vec<PlaybackSession> {
        self.sessions.iter().map(|e| e.value().clone()).collect()
    }

    /// Update a session in-place via a closure.
    pub fn update<F: FnOnce(&mut PlaybackSession)>(&self, id: &str, f: F) {
        if let Some(mut entry) = self.sessions.get_mut(id) {
            f(entry.value_mut());
        }
    }

    /// Update `last_activity` on the session.
    pub fn ping(&self, id: &str) {
        self.update(id, |s| s.last_activity = Utc::now());
    }

    /// Attach a transcode session. If no playback session exists yet (the client
    /// calls master.m3u8 before POST /sessions/playing), a stub is inserted so the
    /// transcode isn't lost. `insert` will later overwrite the stub fields while
    /// preserving the transcode.
    ///
    /// When creating a stub, we inherit the `device_id` from the most recently
    /// active session that currently has no transcode. This covers quality-switch
    /// flows where the client sends a new play_session_id without first calling
    /// POST /Sessions/Playing — without this, `get_sessions` can't find the new
    /// transcode by device_id until the client reports playback again.
    pub fn attach_transcode(
        &self,
        id: &str,
        ts: Arc<tokio::sync::RwLock<TranscodeSession>>,
    ) {
        if let Some(mut entry) = self.sessions.get_mut(id) {
            entry.value_mut().transcode = Some(ts);
        } else {
            // Inherit device_id from the freshest transcodeless session so that
            // get_sessions can find this stub by device_id immediately.
            let inherited_device_id = self
                .sessions
                .iter()
                .filter(|e| {
                    e.value().transcode.is_none() && !e.value().device_id.is_empty()
                })
                .max_by_key(|e| e.value().last_activity)
                .map(|e| e.value().device_id.clone())
                .unwrap_or_default();

            self.sessions.insert(
                id.to_string(),
                PlaybackSession {
                    play_session_id: id.to_string(),
                    transcode: Some(ts),
                    user_id: Uuid::nil(),
                    item_id: Uuid::nil(),
                    media_source_id: None,
                    device_id: inherited_device_id,
                    client_name: String::new(),
                    position_ticks: 0,
                    can_seek: true,
                    is_paused: false,
                    last_paused_at: None,
                    is_muted: false,
                    volume_level: None,
                    audio_stream_index: None,
                    subtitle_stream_index: None,
                    play_method: None,
                    now_playing_queue: None,
                    playlist_item_id: None,
                    started_at: Utc::now(),
                    last_activity: Utc::now(),
                },
            );
        }
    }

    /// Stop and remove the transcode from a session (e.g. on seek or client stop),
    /// but keep the playback session itself alive.
    pub async fn stop_transcode(&self, id: &str) {
        let ts = self
            .sessions
            .get_mut(id)
            .and_then(|mut e| e.value_mut().transcode.take());
        if let Some(ts) = ts {
            kill_transcode(ts).await;
        }
    }

    /// Stop the transcode (if any) and remove the playback session entirely.
    /// Returns the removed session so callers can read final position/item data.
    pub async fn stop(&self, id: &str) -> Option<PlaybackSession> {
        let (_, session) = self.sessions.remove(id)?;
        if let Some(ts) = session.transcode.clone() {
            kill_transcode(ts).await;
        }
        Some(session)
    }

    /// Path where a given HLS segment lives on disk (used for disk-based recovery).
    pub fn segment_path(&self, play_session_id: &str, segment_id: &str) -> PathBuf {
        let session_dir = self.base_dir.join(play_session_id);

        if let Ok(entries) = std::fs::read_dir(&session_dir) {
            let mut latest_dir: Option<(PathBuf, std::time::SystemTime)> = None;
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        let modified = metadata
                            .modified()
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        if latest_dir.as_ref().map_or(true, |(_, max)| modified > *max)
                        {
                            latest_dir = Some((entry.path(), modified));
                        }
                    }
                }
            }
            if let Some((dir, _)) = latest_dir {
                return dir.join(format!("{}.ts", segment_id));
            }
        }

        session_dir.join(format!("{}.ts", segment_id))
    }

    pub fn base_dir(&self) -> &std::path::Path {
        &self.base_dir
    }

    pub fn active_session_ids(&self) -> Vec<String> {
        self.sessions.iter().map(|e| e.key().clone()).collect()
    }

    /// Spawn a background task that reaps sessions idle longer than `max_age`.
    pub fn spawn_cleanup_task(
        self,
        interval: Duration,
        max_age: Duration,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let cutoff = Utc::now()
                    - chrono::Duration::from_std(max_age).unwrap_or_default();
                let stale: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|e| e.value().last_activity < cutoff)
                    .map(|e| e.key().clone())
                    .collect();
                for id in stale {
                    info!("Cleaning up idle session: {}", id);
                    self.stop(&id).await;
                }
            }
        })
    }
}

/// Kill an ffmpeg process and wait for it to exit before returning.
async fn kill_transcode(ts: Arc<tokio::sync::RwLock<TranscodeSession>>) {
    let (kill_tx, wait_done, output_dir) = {
        let mut s = ts.write().await;
        (s.kill_tx.take(), s.wait_done.clone(), s.output_dir.clone())
    };
    if let Some(kill_tx) = kill_tx {
        let notification = wait_done.notified();
        let _ = kill_tx.send(());
        notification.await;
    }
    let _ = std::fs::remove_dir_all(&output_dir);
}
