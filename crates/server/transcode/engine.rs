use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error, debug};

use super::session::{TranscodeSession, TranscodeState};

/// Parameters for starting a new HLS transcode job.
#[derive(Debug, Clone)]
pub struct TranscodeParams {
    pub input_url: String,
    pub output_dir: PathBuf,
    pub video_codec: String,         // "copy", "libx264", "libx265"
    pub audio_codec: String,         // "aac", "copy"
    pub segment_length: u32,         // seconds (default 6)
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

impl Default for TranscodeParams {
    fn default() -> Self {
        Self {
            input_url: String::new(),
            output_dir: PathBuf::new(),
            video_codec: "copy".to_string(),
            audio_codec: "aac".to_string(),
            segment_length: 6,
            start_time_ticks: None,
            max_width: None,
            max_height: None,
            video_bitrate: None,
            audio_bitrate: None,
            audio_channels: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
        }
    }
}

/// Start an HLS transcode job using ez-ffmpeg.
///
/// This spawns the actual FFmpeg work on a blocking thread (CPU-bound)
/// and updates the session state accordingly.
pub async fn start_transcode(
    session: Arc<RwLock<TranscodeSession>>,
    params: TranscodeParams,
) -> Result<()> {
    use crate::ez_ffmpeg::{FfmpegContext, FfmpegScheduler, Input, Output};

    // Update state to Running
    {
        let mut s = session.write().await;
        s.state = TranscodeState::Running;
    }

    let session_clone = session.clone();
    let params_clone = params.clone();

    // Spawn blocking because FFmpeg transcoding is CPU-intensive
    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let output_dir = &params_clone.output_dir;
        std::fs::create_dir_all(output_dir)?;

        let playlist_path = output_dir.join("main.m3u8");
        let segment_pattern = output_dir
            .join("segment_%05d.ts")
            .to_string_lossy()
            .to_string();

        // Build the input
        let mut input = Input::from(params_clone.input_url.as_str());

        // If start time specified, seek to it
        // Jellyfin ticks: 1 tick = 100 nanoseconds = 10_000_000 ticks/second
        if let Some(ticks) = params_clone.start_time_ticks {
            let seconds = ticks as f64 / 10_000_000.0;
            input = Input::from(params_clone.input_url.as_str())
                .set_input_opt("ss", format!("{:.3}", seconds));
        }

        // Build the output with HLS options
        let mut output = Output::from(playlist_path.to_str().unwrap())
            .set_format("hls")
            .set_format_opt("hls_time", &params_clone.segment_length.to_string())
            .set_format_opt("hls_segment_filename", &segment_pattern)
            .set_format_opt("hls_flags", "independent_segments+append_list")
            .set_format_opt("hls_list_size", "0")   // keep all segments in playlist
            .set_format_opt("hls_segment_type", "mpegts");

        // Video codec
        output = match params_clone.video_codec.as_str() {
            "copy" => output.set_video_codec("copy"),
            codec => {
                let mut o = output.set_video_codec(codec);
                // Set quality/bitrate
                if let Some(bitrate) = params_clone.video_bitrate {
                    o = o.set_video_codec_opt("b", &format!("{}k", bitrate / 1000));
                } else {
                    // Default CRF for quality
                    o = o.set_video_codec_opt("crf", "23");
                }
                // Max dimensions via scale filter handled below
                o
            }
        };

        // Audio codec
        output = match params_clone.audio_codec.as_str() {
            "copy" => output.set_audio_codec("copy"),
            codec => {
                let mut o = output.set_audio_codec(codec);
                if let Some(bitrate) = params_clone.audio_bitrate {
                    o = o.set_audio_codec_opt("b", &format!("{}k", bitrate / 1000));
                } else {
                    o = o.set_audio_codec_opt("b", "128k");
                }
                if let Some(channels) = params_clone.audio_channels {
                    o = o.set_audio_codec_opt("ac", &channels.to_string());
                }
                o
            }
        };

        // Build filter for scaling if needed
        let mut builder = FfmpegContext::builder()
            .input(input)
            .output(output);

        // Add scale filter if max dimensions specified and video is not copy
        if params_clone.video_codec != "copy" {
            if let (Some(w), Some(h)) = (params_clone.max_width, params_clone.max_height) {
                builder = builder.filter_desc(
                    format!("scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
                             w, h)
                );
            } else if let Some(w) = params_clone.max_width {
                builder = builder.filter_desc(
                    format!("scale='min({},iw)':-2", w)
                );
            } else if let Some(h) = params_clone.max_height {
                builder = builder.filter_desc(
                    format!("scale=-2:'min({},ih)'", h)
                );
            }
        }

        // Audio/subtitle stream selection via map options
        // For now we let FFmpeg auto-select; stream selection can be added later

        let context = builder.build()?;

        info!("Starting HLS transcode job");

        FfmpegScheduler::new(context)
            .start()?
            .wait()?;

        info!("HLS transcode job completed");

        Ok(())
    });

    // Wait for result and update session state
    match handle.await {
        Ok(Ok(())) => {
            let mut s = session.write().await;
            s.state = TranscodeState::Complete;
            info!(session_id = %s.id, "Transcode completed successfully");
        }
        Ok(Err(e)) => {
            let mut s = session.write().await;
            let err_msg = format!("{:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode failed");
            s.state = TranscodeState::Error(err_msg);
        }
        Err(e) => {
            let mut s = session.write().await;
            let err_msg = format!("Task panicked: {:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode task panicked");
            s.state = TranscodeState::Error(err_msg);
        }
    }

    Ok(())
}

/// Parameters for a progressive (non-HLS) transcode that streams to stdout.
#[derive(Debug, Clone)]
pub struct ProgressiveTranscodeParams {
    pub input_url: String,
    pub container: String,           // "mp4", "ts", "mkv", "webm"
    pub video_codec: String,         // "copy", "libx264", "libx265", "libvpx-vp9"
    pub audio_codec: String,         // "copy", "aac", "libopus"
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

/// Start a progressive transcode that returns a readable stream.
///
/// Unlike HLS transcode, this pipes ffmpeg output directly to a reader
/// that can be streamed as an HTTP response body.
pub fn start_progressive_transcode(
    params: ProgressiveTranscodeParams,
) -> Result<tokio::process::ChildStdout> {
    use tokio::process::Command;

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel").arg("warning");

    // Seek if start time specified
    if let Some(ticks) = params.start_time_ticks {
        let seconds = ticks as f64 / 10_000_000.0;
        cmd.arg("-ss").arg(format!("{:.3}", seconds));
    }

    cmd.arg("-i").arg(&params.input_url);

    // Video codec
    cmd.arg("-c:v").arg(&params.video_codec);
    if params.video_codec != "copy" {
        if let Some(bitrate) = params.video_bitrate {
            cmd.arg("-b:v").arg(format!("{}k", bitrate / 1000));
        } else {
            cmd.arg("-crf").arg("23");
        }
        // Scale filter
        match (params.max_width, params.max_height) {
            (Some(w), Some(h)) => {
                cmd.arg("-vf").arg(format!(
                    "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
                    w, h
                ));
            }
            (Some(w), None) => {
                cmd.arg("-vf").arg(format!("scale='min({},iw)':-2", w));
            }
            (None, Some(h)) => {
                cmd.arg("-vf").arg(format!("scale=-2:'min({},ih)'", h));
            }
            _ => {}
        }
    }

    // Audio codec
    cmd.arg("-c:a").arg(&params.audio_codec);
    if params.audio_codec != "copy" {
        if let Some(bitrate) = params.audio_bitrate {
            cmd.arg("-b:a").arg(format!("{}k", bitrate / 1000));
        } else {
            cmd.arg("-b:a").arg("128k");
        }
        if let Some(channels) = params.audio_channels {
            cmd.arg("-ac").arg(channels.to_string());
        }
    }

    // Audio stream selection
    if let Some(idx) = params.audio_stream_index {
        cmd.arg("-map").arg("0:v:0");
        cmd.arg("-map").arg(format!("0:a:{}", idx));
    }

    // Container-specific options for streaming
    let format = match params.container.as_str() {
        "ts" | "mpegts" => "mpegts",
        "webm" => "webm",
        "mkv" | "matroska" => "matroska",
        _ => "mp4",  // default to mp4
    };
    cmd.arg("-f").arg(format);

    // For mp4 streaming, need movflags for fragmented output
    if format == "mp4" {
        cmd.arg("-movflags").arg("frag_keyframe+empty_moov+faststart");
    }

    cmd.arg("-")  // output to stdout
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    info!("Starting progressive transcode: {:?}", cmd);

    let mut child = cmd.spawn()
        .map_err(|e| anyhow!("Failed to spawn ffmpeg: {}", e))?;

    let stdout = child.stdout.take()
        .ok_or_else(|| anyhow!("Failed to capture ffmpeg stdout"))?;

    // Spawn a task to wait for the child process so it doesn't become a zombie
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) if !status.success() => {
                error!("Progressive transcode exited with status: {}", status);
            }
            Err(e) => {
                error!("Failed to wait on ffmpeg process: {}", e);
            }
            _ => {
                debug!("Progressive transcode completed successfully");
            }
        }
    });

    Ok(stdout)
}

/// Generate a master HLS playlist that references the variant playlist.
/// This mimics Jellyfin's master.m3u8 format.
pub fn generate_master_playlist(session: &TranscodeSession) -> String {
    let play_session_id = &session.id;
    format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-STREAM-INF:BANDWIDTH=2000000\n\
         main.m3u8?PlaySessionId={}\n",
        play_session_id
    )
}
