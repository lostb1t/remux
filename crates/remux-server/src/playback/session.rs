use remux_sdks::remux::TranscodeReasons;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicU32},
    time::Instant,
};
use tokio::sync::{Notify, watch};
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
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub burn_subtitle: bool,
    pub segment_length: u32,
    pub transcode_reasons: TranscodeReasons,
    /// Kill channel and done notifier for the ffmpeg subprocess.
    pub kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub wait_done: Arc<Notify>,
    /// Index of the last segment the client has requested (0-based).
    pub last_segment_index: Arc<AtomicU32>,
    /// Start offset of this transcode in seconds (from start_time_ticks).
    pub start_time_secs: u32,
    /// Playback offset in seconds relative to start_time_secs, updated from progress reports.
    pub playback_offset_secs: Arc<AtomicU32>,
    /// Total runtime of the media in Jellyfin ticks (100-ns units).
    pub runtime_ticks: i64,
    /// Whether runtime_ticks came from probing the selected media source.
    /// Unprobed runtimes are metadata estimates and must not define a VOD timeline.
    pub runtime_is_probed: bool,
    /// True for live TV — variant playlist is served from the ffmpeg-written EVENT file.
    pub is_live: bool,
    /// Codec name of the source video stream (e.g. "hevc", "h264"), used when
    /// rebuilding `TranscodeParams` on seek-restart.
    pub source_video_codec: Option<String>,
    /// Codec name of the source audio stream (e.g. "eac3", "aac").
    pub source_audio_codec: Option<String>,
    /// Profile of the source video stream (e.g. "Main 10"), used to generate
    /// the correct HLS CODECS attribute string for HEVC.
    pub source_video_profile: Option<String>,
    /// Level of the source video stream (e.g. 150.0 for level 5.0).
    pub source_video_level: Option<f64>,
    /// HDR type of the source video (SDR/HDR10/HLG/…), used for VIDEO-RANGE in master playlist.
    pub source_video_range_type: Option<remux_sdks::remux::VideoRangeType>,
    /// Source video width in pixels, used for RESOLUTION in master playlist.
    pub source_video_width: Option<i64>,
    /// Source video height in pixels, used for RESOLUTION in master playlist.
    pub source_video_height: Option<i64>,
    /// Source video frame rate (fps), used for FRAME-RATE in master playlist.
    pub source_frame_rate: Option<f32>,
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
        audio_stream_index: Option<i32>,
        subtitle_stream_index: Option<i32>,
        burn_subtitle: bool,
        segment_length: u32,
        transcode_reasons: TranscodeReasons,
        runtime_ticks: i64,
        runtime_is_probed: bool,
        is_live: bool,
        source_video_codec: Option<String>,
        source_audio_codec: Option<String>,
        source_video_profile: Option<String>,
        source_video_level: Option<f64>,
        source_video_range_type: Option<remux_sdks::remux::VideoRangeType>,
        source_video_width: Option<i64>,
        source_video_height: Option<i64>,
        source_frame_rate: Option<f32>,
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
            audio_stream_index,
            subtitle_stream_index,
            burn_subtitle,
            segment_length,
            transcode_reasons,
            kill_tx: None,
            wait_done: Arc::new(Notify::new()),
            last_segment_index: Arc::new(AtomicU32::new(0)),
            start_time_secs: 0,
            playback_offset_secs: Arc::new(AtomicU32::new(0)),
            runtime_ticks,
            runtime_is_probed,
            is_live,
            source_video_codec,
            source_audio_codec,
            source_video_profile,
            source_video_level,
            source_video_range_type,
            source_video_width,
            source_video_height,
            source_frame_rate,
        }))
    }

    pub fn master_playlist_path(&self) -> PathBuf {
        self.output_dir
            .join("master.m3u8")
    }

    pub fn variant_playlist_path(&self) -> PathBuf {
        self.output_dir
            .join("main.m3u8")
    }

    /// Returns true if this session should use fragmented MP4 (fMP4) segments
    /// rather than MPEG-TS. iOS Safari (and the HLS spec) require fMP4 for HEVC.
    pub fn use_fmp4(&self) -> bool {
        self.video_codec == "copy"
            && matches!(
                self.source_video_codec
                    .as_deref(),
                Some("hevc") | Some("h265") | Some("hvc1") | Some("hev1")
            )
    }

    pub fn segment_path(&self, segment_id: &str) -> PathBuf {
        let ext = if self.use_fmp4() { "m4s" } else { "ts" };
        self.output_dir
            .join(format!("{}.{}", segment_id, ext))
    }

    /// Path of the fMP4 initialization segment written by ffmpeg.
    pub fn init_segment_path(&self) -> PathBuf {
        self.output_dir
            .join("init.mp4")
    }

    /// Generate a Jellyfin-compatible transcoding URL
    pub fn transcoding_url(&self) -> String {
        format!(
            "/videos/{}/master.m3u8?PlaySessionId={}&VideoCodec={}&AudioCodec=aac&SegmentContainer=ts&SegmentLength={}&MediaSourceId={}",
            self.item_id
                .as_simple(),
            self.id,
            self.video_codec,
            self.segment_length,
            self.media_source_id
                .as_simple(),
        )
    }
}
