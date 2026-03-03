use dashmap::DashMap;
use shared::sdks::jellyfin::models::TranscodeReasons;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum TranscodeState {
    Starting,
    Running,
    Complete,
    Error(String),
}

pub struct TranscodeSession {
    pub id: String,
    pub item_id: Uuid,
    pub media_source_id: Uuid,
    pub output_dir: PathBuf,
    pub input_url: String,
    pub state: TranscodeState,
    pub created_at: Instant,
    pub last_accessed: Instant,
    pub video_codec: String,
    pub audio_codec: String,
    pub segment_length: u32,
    pub transcode_reasons: TranscodeReasons,
}

impl TranscodeSession {
    pub fn master_playlist_path(&self) -> PathBuf {
        self.output_dir.join("master.m3u8")
    }

    pub fn variant_playlist_path(&self) -> PathBuf {
        self.output_dir.join("main.m3u8")
    }

    pub fn segment_path(&self, segment_id: &str) -> PathBuf {
        self.output_dir.join(format!("{}.ts", segment_id))
    }

    /// Generate a Jellyfin-compatible transcoding URL
    pub fn transcoding_url(&self) -> String {
        format!(
            "/videos/{}/master.m3u8?PlaySessionId={}&VideoCodec={}&AudioCodec=aac&SegmentContainer=ts&SegmentLength={}&MediaSourceId={}",
            self.item_id.as_simple(),
            self.id,
            self.video_codec,
            self.segment_length,
            self.media_source_id.as_simple(),
        )
    }
}

#[derive(Clone)]
pub struct TranscodeSessionManager {
    sessions: Arc<DashMap<String, Arc<tokio::sync::RwLock<TranscodeSession>>>>,
    base_dir: PathBuf,
}

impl TranscodeSessionManager {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        // ensure base dir exists
        let _ = std::fs::create_dir_all(&base_dir);
        Self {
            sessions: Arc::new(DashMap::new()),
            base_dir,
        }
    }

    pub fn create(
        &self,
        play_session_id: String,
        item_id: Uuid,
        media_source_id: Uuid,
        input_url: String,
        video_codec: String,
        audio_codec: String,
        segment_length: u32,
        transcode_reasons: TranscodeReasons,
    ) -> Arc<tokio::sync::RwLock<TranscodeSession>> {
        let output_dir = self.base_dir.join(&play_session_id);
        let _ = std::fs::create_dir_all(&output_dir);

        let session = TranscodeSession {
            id: play_session_id.clone(),
            item_id,
            media_source_id,
            output_dir,
            input_url,
            state: TranscodeState::Starting,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            video_codec,
            audio_codec,
            segment_length,
            transcode_reasons,
        };

        let session = Arc::new(tokio::sync::RwLock::new(session));
        self.sessions.insert(play_session_id, session.clone());
        session
    }

    pub fn get(
        &self,
        play_session_id: &str,
    ) -> Option<Arc<tokio::sync::RwLock<TranscodeSession>>> {
        self.sessions
            .get(play_session_id)
            .map(|s| s.value().clone())
    }

    pub async fn stop(&self, play_session_id: &str) {
        if let Some((_, session)) = self.sessions.remove(play_session_id) {
            let session = session.read().await;
            // Clean up files
            let _ = std::fs::remove_dir_all(&session.output_dir);
        }
    }

    pub async fn cleanup_stale(&self, max_age: Duration) {
        let now = Instant::now();
        let stale_ids: Vec<String> = {
            let mut ids = Vec::new();
            for entry in self.sessions.iter() {
                let session = entry.value().read().await;
                if now.duration_since(session.last_accessed) > max_age {
                    ids.push(entry.key().clone());
                }
            }
            ids
        };

        for id in &stale_ids {
            tracing::info!("Cleaning up stale transcode session: {}", id);
            self.stop(id).await;
        }
    }

    /// Spawn a background task that cleans up stale sessions periodically
    pub fn spawn_cleanup_task(
        self,
        interval: Duration,
        max_age: Duration,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.cleanup_stale(max_age).await;
            }
        })
    }
}
