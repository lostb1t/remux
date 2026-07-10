use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{common, db, db::auth, playback::session::TranscodeSession};
use remux_sdks::remux::{PlayMethod, PlaybackInfo, QueueItem};

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
    /// Stream group UUID that the selected source belongs to, if any.
    pub group_id: Option<Uuid>,
    /// Kind of the item being played, used to populate NowPlayingItem in session broadcasts.
    pub item_kind: Option<db::MediaKind>,
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

    /// Handle a `POST /sessions/playing` report.
    ///
    /// Enforces the per-user session limit, resolves the optional StreamGroup
    /// source, builds and inserts the `PlaybackSession`, and emits the playback-
    /// start log line (skipped for transcode — the HLS handler logs that after
    /// it has codec/bitrate/reason details).
    pub async fn start(
        &self,
        db: &sqlx::SqlitePool,
        auth_session: &auth::AuthSession,
        data: &PlaybackInfo,
    ) -> anyhow::Result<()> {
        let play_session_id = data
            .play_session_id
            .clone()
            .unwrap_or_else(|| {
                common::get_uuid()
                    .as_simple()
                    .to_string()
            });

        // Enforce per-user concurrent-stream limit.
        let max_sessions = auth_session
            .user
            .policy
            .as_ref()
            .map(|p| p.max_active_sessions)
            .unwrap_or(0);
        if max_sessions > 0 {
            // Exclude the caller's own device: insert() will replace any existing
            // session for that device, so it doesn't consume an extra slot.
            let current = self.count_for_user(
                auth_session
                    .user
                    .id,
                Some(
                    &auth_session
                        .device
                        .id,
                ),
            );
            if current >= max_sessions as usize {
                return Err(anyhow::anyhow!("Stream limit reached")
                    .context("Maximum concurrent streams reached"));
            }
        }

        let item_id = data.item_id;
        if item_id.is_nil() {
            warn!(
                client = %auth_session.device.app_name,
                "PlaybackStart missing item_id, skipping session creation"
            );
            return Ok(());
        }

        // If the client selected a StreamGroup source, record its group UUID.
        let group_id: Option<Uuid> = if let Some(ref sid) = data.media_source_id {
            if let Ok(uid) = sid.parse::<Uuid>() {
                db::Media::get_by_id(db, &uid)
                    .await
                    .ok()
                    .flatten()
                    .filter(|m| m.kind == db::MediaKind::StreamGroup)
                    .map(|_| uid)
            } else {
                None
            }
        } else {
            None
        };

        let item_kind = db::Media::get_by_id(db, &item_id)
            .await
            .ok()
            .flatten()
            .map(|m| m.kind);

        let ps = PlaybackSession {
            play_session_id: play_session_id.clone(),
            user_id: auth_session
                .user
                .id,
            item_id,
            media_source_id: data
                .media_source_id
                .clone(),
            device_id: auth_session
                .device
                .id
                .clone(),
            client_name: auth_session
                .device
                .app_name
                .clone(),
            position_ticks: data
                .position_ticks
                .unwrap_or(0),
            can_seek: data.can_seek,
            is_paused: data.is_paused,
            last_paused_at: if data.is_paused {
                Some(Utc::now())
            } else {
                None
            },
            is_muted: data.is_muted,
            volume_level: data.volume_level,
            audio_stream_index: data.audio_stream_index,
            subtitle_stream_index: data.subtitle_stream_index,
            play_method: data
                .play_method
                .as_ref()
                .map(|m| m.to_string()),
            now_playing_queue: data
                .now_playing_queue
                .clone(),
            playlist_item_id: data
                .playlist_item_id
                .clone(),
            started_at: Utc::now(),
            last_activity: Utc::now(),
            transcode: None,
            group_id,
            item_kind,
        };

        self.insert(ps);

        // For transcode sessions, master_hls_video fires the info log once it
        // has full codec/bitrate/reasons info. For direct play/stream, log here.
        let is_transcode = matches!(data.play_method, Some(PlayMethod::Transcode));
        if !is_transcode {
            // Best-effort: fetch media title and source path for the log line.
            let media_title = db::Media::get_by_id(db, &item_id)
                .await
                .ok()
                .flatten()
                .map(|m| m.title)
                .unwrap_or_default();

            let (source_title, source_path) =
                if let Some(ref sid) = data.media_source_id {
                    if let Ok(source_uuid) = sid.parse::<Uuid>() {
                        let m = db::Media::get_by_id(db, &source_uuid)
                            .await
                            .ok()
                            .flatten();
                        (
                            m.as_ref()
                                .map(|m| {
                                    m.title
                                        .clone()
                                }),
                            m.and_then(|m| {
                                m.stream_info
                                    .map(|si| si.descriptor)
                            }),
                        )
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

            let log_session_id = play_session_id
                .trim_start_matches("audio-")
                .trim_start_matches("video-");
            let position_secs = data
                .position_ticks
                .unwrap_or(0)
                / 10_000_000;
            info!(
                play_session_id = log_session_id,
                %item_id,
                title = %media_title,
                source = ?source_title,
                path = ?source_path,
                user = %auth_session.user.username,
                client = %auth_session.device.app_name,
                play_method = ?data.play_method,
                audio_stream = ?data.audio_stream_index,
                subtitle_stream = ?data.subtitle_stream_index,
                position_secs,
                "▶ Playback started"
            );
        }

        Ok(())
    }

    /// Handle a `POST /sessions/playing/progress` report.
    ///
    /// Updates the in-memory session state, notifies the transcode buffer
    /// monitor, persists the playback position to the DB, and logs stream-
    /// selection changes.
    pub async fn progress(
        &self,
        db: &sqlx::SqlitePool,
        user: &db::User,
        psid: &str,
        data: &PlaybackInfo,
    ) -> anyhow::Result<()> {
        let ps_snapshot = self.get(psid);
        let ps = match ps_snapshot.as_ref() {
            Some(ps) => ps,
            None => return Ok(()),
        };

        // If the session has no valid item and the progress report can't supply one,
        // it's a ghost (e.g. an unclaimed transcode stub). Evict it instead of
        // keeping it alive via last_activity.
        if ps
            .item_id
            .is_nil()
            && data
                .item_id
                .is_nil()
        {
            self.sessions
                .remove(psid);
            return Ok(());
        }

        let item_id = if !data
            .item_id
            .is_nil()
        {
            data.item_id
        } else {
            ps.item_id
        };

        // Detect encode-parameter changes and log them once.
        // We ignore pause/unpause — those are not encode changes.
        let audio_changed = data
            .audio_stream_index
            .is_some()
            && data.audio_stream_index != ps.audio_stream_index;
        let subtitle_changed = data
            .subtitle_stream_index
            .is_some()
            && data.subtitle_stream_index != ps.subtitle_stream_index;
        let method_changed = data
            .play_method
            .is_some()
            && data
                .play_method
                .as_ref()
                .map(|m| m.to_string())
                != ps.play_method;
        if audio_changed || subtitle_changed || method_changed {
            info!(
                play_session_id = psid.trim_start_matches("audio-").trim_start_matches("video-"),
                item_id = %item_id,
                user = %user.username,
                audio_stream = if audio_changed {
                    format!("{:?} → {:?}", ps.audio_stream_index, data.audio_stream_index)
                } else {
                    format!("{:?}", ps.audio_stream_index)
                },
                subtitle_stream = if subtitle_changed {
                    format!("{:?} → {:?}", ps.subtitle_stream_index, data.subtitle_stream_index)
                } else {
                    format!("{:?}", ps.subtitle_stream_index)
                },
                play_method = if method_changed {
                    format!("{:?} → {:?}", ps.play_method, data.play_method)
                } else {
                    format!("{:?}", ps.play_method)
                },
                "⟳ Playback params changed"
            );
        }

        self.update(psid, |ps| {
            if !data
                .item_id
                .is_nil()
            {
                ps.item_id = data.item_id;
            }
            ps.position_ticks = data
                .position_ticks
                .unwrap_or(ps.position_ticks);
            if data.is_paused && !ps.is_paused {
                ps.last_paused_at = Some(Utc::now());
            } else if !data.is_paused {
                ps.last_paused_at = None;
            }
            ps.is_paused = data.is_paused;
            ps.is_muted = data.is_muted;
            ps.volume_level = data
                .volume_level
                .or(ps.volume_level);
            ps.audio_stream_index = data
                .audio_stream_index
                .or(ps.audio_stream_index);
            ps.subtitle_stream_index = data
                .subtitle_stream_index
                .or(ps.subtitle_stream_index);
            if let Some(ref m) = data.play_method {
                ps.play_method = Some(m.to_string());
            }
            ps.last_activity = Utc::now();
        });

        // Update transcode buffer monitor with actual playback position.
        if let Some(position_ticks) = data.position_ticks {
            if let Some(ref ts_lock) = ps.transcode {
                if let Ok(ts) = ts_lock.try_read() {
                    let position_secs = (position_ticks / 10_000_000) as u32;
                    let offset = position_secs.saturating_sub(ts.start_time_secs);
                    ts.playback_offset_secs
                        .store(offset, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        // Persist position to DB (no watched-threshold check on progress).
        let position_ticks = data
            .position_ticks
            .unwrap_or(ps.position_ticks);
        if let Ok(Some(media)) = db::Media::get_by_id(db, &item_id).await {
            let cfg = user
                .configuration
                .as_ref()
                .map(|c| {
                    c.0.clone()
                })
                .unwrap_or_default();

            let audio_idx = if cfg.remember_audio_selections {
                data.audio_stream_index
                    .or(ps.audio_stream_index)
                    .map(|x| x as i64)
            } else {
                None
            };
            let subtitle_idx = if cfg.remember_subtitle_selections {
                data.subtitle_stream_index
                    .or(ps.subtitle_stream_index)
                    .map(|x| x as i64)
            } else {
                None
            };

            db::UserMediaState::update_playback(
                db,
                user,
                &media,
                position_ticks,
                audio_idx,
                subtitle_idx,
                None, // no watched-threshold check on progress
            )
            .await?;
        }

        Ok(())
    }

    /// Handle a `POST /sessions/playing/stopped` report.
    ///
    /// Removes the playback session (stopping any active transcode), persists
    /// the final position to the DB (with the 90 % watched-mark check), and
    /// emits a debug log line.
    pub async fn stopped(
        &self,
        db: &sqlx::SqlitePool,
        user: &db::User,
        psid: &str,
        data: &PlaybackInfo,
    ) -> anyhow::Result<()> {
        let ps = self
            .stop(psid)
            .await;

        let item_id = Some(data.item_id)
            .filter(|id| !id.is_nil())
            .or_else(|| {
                ps.as_ref()
                    .map(|s| s.item_id)
            });
        let final_ticks = data
            .position_ticks
            .or_else(|| {
                ps.as_ref()
                    .map(|s| s.position_ticks)
            });

        if let Some(item_id) = item_id {
            if let Ok(Some(media)) = db::Media::get_by_id(db, &item_id).await {
                db::UserMediaState::update_playback(
                    db,
                    user,
                    &media,
                    final_ticks.unwrap_or(0),
                    None, // don't overwrite stream selections on stop
                    None,
                    media.runtime, // Some(runtime) triggers watched-threshold check
                )
                .await?;
            }
        }

        debug!(play_session_id = psid, "Playback stopped");
        Ok(())
    }

    /// Insert (or replace) a playback session, preserving any transcode that was
    /// pre-attached before `report_playback_start` fired.
    /// Removes stale sessions for the same device so `get_sessions` always
    /// finds the most recent playback.
    pub fn insert(&self, mut session: PlaybackSession) {
        if session
            .transcode
            .is_none()
        {
            if let Some(existing) = self
                .sessions
                .get(&session.play_session_id)
            {
                session.transcode = existing
                    .value()
                    .transcode
                    .clone();
            }
        }
        // Remove any previous session for this device (different play_session_id).
        if !session
            .device_id
            .is_empty()
        {
            let stale: Vec<String> = self
                .sessions
                .iter()
                .filter(|e| {
                    e.value()
                        .device_id
                        == session.device_id
                        && e.key() != &session.play_session_id
                })
                .map(|e| {
                    e.key()
                        .clone()
                })
                .collect();
            for id in stale {
                self.sessions
                    .remove(&id);
            }
        }
        self.sessions
            .insert(
                session
                    .play_session_id
                    .clone(),
                session,
            );
    }

    /// Return a clone of the session, if it exists.
    pub fn get(&self, id: &str) -> Option<PlaybackSession> {
        self.sessions
            .get(id)
            .map(|e| {
                e.value()
                    .clone()
            })
    }

    /// Return a clone of the transcode session attached to this playback session.
    pub fn get_transcode(
        &self,
        id: &str,
    ) -> Option<Arc<tokio::sync::RwLock<TranscodeSession>>> {
        self.sessions
            .get(id)?
            .value()
            .transcode
            .clone()
    }

    /// Return clones of all active sessions.
    pub fn get_all(&self) -> Vec<PlaybackSession> {
        self.sessions
            .iter()
            .map(|e| {
                e.value()
                    .clone()
            })
            .collect()
    }

    /// Return the most recently active session for a device.
    /// Used as a fallback when the client omits PlaySessionId (e.g. DirectPlay).
    pub fn get_by_device(&self, device_id: &str) -> Option<PlaybackSession> {
        self.sessions
            .iter()
            .filter(|e| {
                e.value()
                    .device_id
                    == device_id
            })
            .max_by_key(|e| {
                e.value()
                    .last_activity
            })
            .map(|e| {
                e.value()
                    .clone()
            })
    }

    /// Count active sessions for a user, optionally excluding sessions from a
    /// specific device. Excluding the caller's device is correct when checking
    /// before `insert()`, since insert() replaces any existing session for that
    /// device and it shouldn't count toward the limit.
    pub fn count_for_user(&self, user_id: Uuid, exclude_device: Option<&str>) -> usize {
        self.sessions
            .iter()
            .filter(|e| {
                let s = e.value();
                s.user_id == user_id
                    && exclude_device.map_or(true, |d| s.device_id != d)
            })
            .count()
    }

    /// Update a session in-place via a closure.
    pub fn update<F: FnOnce(&mut PlaybackSession)>(&self, id: &str, f: F) {
        if let Some(mut entry) = self
            .sessions
            .get_mut(id)
        {
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
        if let Some(mut entry) = self
            .sessions
            .get_mut(id)
        {
            entry
                .value_mut()
                .transcode = Some(ts);
        } else {
            // Inherit device_id from the freshest transcodeless session so that
            // get_sessions can find this stub by device_id immediately.
            let inherited_device_id = self
                .sessions
                .iter()
                .filter(|e| {
                    e.value()
                        .transcode
                        .is_none()
                        && !e
                            .value()
                            .device_id
                            .is_empty()
                })
                .max_by_key(|e| {
                    e.value()
                        .last_activity
                })
                .map(|e| {
                    e.value()
                        .device_id
                        .clone()
                })
                .unwrap_or_default();

            self.sessions
                .insert(
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
                        group_id: None,
                        item_kind: None,
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
            .and_then(|mut e| {
                e.value_mut()
                    .transcode
                    .take()
            });
        if let Some(ts) = ts {
            kill_transcode(ts).await;
        }
    }

    /// Stop the transcode (if any) and remove the playback session entirely.
    /// Returns the removed session so callers can read final position/item data.
    pub async fn stop(&self, id: &str) -> Option<PlaybackSession> {
        let (_, session) = self
            .sessions
            .remove(id)?;
        if let Some(ts) = session
            .transcode
            .clone()
        {
            kill_transcode(ts).await;
        }
        Some(session)
    }

    /// Path where a given HLS segment lives on disk (used for disk-based recovery).
    pub fn segment_path(&self, play_session_id: &str, segment_id: &str) -> PathBuf {
        let session_dir = self
            .base_dir
            .join(play_session_id);

        if let Ok(entries) = std::fs::read_dir(&session_dir) {
            let mut latest_dir: Option<(PathBuf, std::time::SystemTime)> = None;
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        let modified = metadata
                            .modified()
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        if latest_dir
                            .as_ref()
                            .map_or(true, |(_, max)| modified > *max)
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
        self.sessions
            .iter()
            .map(|e| {
                e.key()
                    .clone()
            })
            .collect()
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
                ticker
                    .tick()
                    .await;
                let cutoff = Utc::now()
                    - chrono::Duration::from_std(max_age).unwrap_or_default();
                let stale: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|e| {
                        e.value()
                            .last_activity
                            < cutoff
                    })
                    .map(|e| {
                        e.key()
                            .clone()
                    })
                    .collect();
                for id in stale {
                    info!("Cleaning up idle session: {}", id);
                    self.stop(&id)
                        .await;
                }
            }
        })
    }
}

/// Kill an ffmpeg process and wait for it to exit before returning.
async fn kill_transcode(ts: Arc<tokio::sync::RwLock<TranscodeSession>>) {
    let (kill_tx, wait_done, output_dir) = {
        let mut s = ts
            .write()
            .await;
        (
            s.kill_tx
                .take(),
            s.wait_done
                .clone(),
            s.output_dir
                .clone(),
        )
    };
    if let Some(kill_tx) = kill_tx {
        let notification = wait_done.notified();
        let _ = kill_tx.send(());
        notification.await;
    }
    let _ = std::fs::remove_dir_all(&output_dir);
}
