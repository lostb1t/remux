use remux_sdks::jellyfin::models::TranscodeReasons;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, Notify};
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
    /// Broadcasts state transitions so waiters can react immediately.
    pub state_tx: Arc<watch::Sender<TranscodeState>>,
    pub created_at: Instant,
    pub video_codec: String,
    pub audio_codec: String,
    pub segment_length: u32,
    pub transcode_reasons: TranscodeReasons,
    /// Kill channel and done notifier for the ffmpeg subprocess.
    pub kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub wait_done: Arc<Notify>,
    /// Index of the last segment the client has requested (0-based).
    pub last_segment_index: Arc<AtomicU32>,
}

impl TranscodeSession {
    pub fn new(
        play_session_id: String,
        item_id: Uuid,
        media_source_id: Uuid,
        input_url: String,
        output_dir: PathBuf,
        video_codec: String,
        audio_codec: String,
        segment_length: u32,
        transcode_reasons: TranscodeReasons,
    ) -> Arc<tokio::sync::RwLock<Self>> {
        let _ = std::fs::create_dir_all(&output_dir);
        let (state_tx, _) = watch::channel(TranscodeState::Starting);
        Arc::new(tokio::sync::RwLock::new(Self {
            id: play_session_id,
            item_id,
            media_source_id,
            output_dir,
            input_url,
            state: TranscodeState::Starting,
            state_tx: Arc::new(state_tx),
            created_at: Instant::now(),
            video_codec,
            audio_codec,
            segment_length,
            transcode_reasons,
            kill_tx: None,
            wait_done: Arc::new(Notify::new()),
            last_segment_index: Arc::new(AtomicU32::new(0)),
        }))
    }

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
