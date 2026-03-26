use anyhow::{Result, anyhow};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::session::{TranscodeSession, TranscodeState};

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}

/// Parameters for starting a new HLS transcode job.
#[derive(Debug, Clone)]
pub struct TranscodeParams {
    pub input_url: String,
    pub output_dir: PathBuf,
    pub video_codec: String, // "copy", "libx264", "libx265"
    pub audio_codec: String, // "aac", "copy"
    pub segment_length: u32, // seconds (default 6)
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

/// Build a scale filter string for FFmpeg, if needed.
fn build_scale_filter(params: &TranscodeParams) -> Option<String> {
    match (params.max_width, params.max_height) {
        (Some(w), Some(h)) => Some(format!(
            "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
            w, h
        )),
        (Some(w), None) => Some(format!("scale='min({},iw)':-2", w)),
        (None, Some(h)) => Some(format!("scale=-2:'min({},ih)'", h)),
        _ => None,
    }
}

/// Build the ffmpeg CLI args for an HLS transcode.
fn build_hls_args(params: &TranscodeParams) -> Vec<String> {
    let ffmpeg_video_codec = match params.video_codec.as_str() {
        "copy" | "h264" | "libx264" => "libx264",
        "hevc" | "libx265" | "h265" => "libx265",
        other => other,
    };
    let ffmpeg_audio_codec = "aac";

    let mut args: Vec<String> = vec![
        "-v".into(), "error".into(),
        "-analyzeduration".into(), "1000000".into(),
        "-probesize".into(), "1000000".into(),
    ];

    // Input seek (fast, before -i)
    if let Some(ticks) = params.start_time_ticks {
        let secs = ticks as f64 / 10_000_000.0;
        args.extend(["-ss".into(), format!("{:.6}", secs)]);
    }

    args.extend(["-i".into(), params.input_url.clone()]);

    // Stream mapping
    let scale_filter = build_scale_filter(params);

    if let Some(ref filter) = scale_filter {
        args.extend(["-vf".into(), filter.clone()]);
    } else if let Some(audio_idx) = params.audio_stream_index {
        args.extend([
            "-map".into(), "0:v".into(),
            "-map".into(), format!("0:{}", audio_idx),
        ]);
    }

    // Video codec
    args.extend(["-c:v".into(), ffmpeg_video_codec.into()]);

    if ffmpeg_video_codec == "libx264" {
        args.extend(["-profile:v".into(), "high".into()]);
        if let Some(bitrate) = params.video_bitrate {
            args.extend(["-b:v".into(), bitrate.to_string()]);
        } else {
            args.extend([
                "-crf".into(), "23".into(),
                "-preset".into(), "fast".into(),
                "-tune".into(), "zerolatency".into(),
            ]);
        }
    } else if let Some(bitrate) = params.video_bitrate {
        args.extend(["-b:v".into(), bitrate.to_string()]);
    }

    // Audio codec
    args.extend(["-c:a".into(), ffmpeg_audio_codec.into()]);
    let audio_bitrate = params.audio_bitrate.unwrap_or(128_000);
    args.extend(["-b:a".into(), audio_bitrate.to_string()]);
    if let Some(ch) = params.audio_channels {
        args.extend(["-ac".into(), ch.to_string()]);
    }

    // HLS output
    let playlist = params.output_dir.join("main.m3u8");
    let segment = params.output_dir.join("segment_%05d.ts");
    args.extend([
        "-f".into(), "hls".into(),
        "-hls_time".into(), params.segment_length.to_string(),
        "-hls_segment_filename".into(), segment.to_string_lossy().into_owned(),
        "-hls_playlist_type".into(), "event".into(),
        "-hls_list_size".into(), "0".into(),
        playlist.to_string_lossy().into_owned(),
    ]);

    args
}

/// Start an HLS transcode job by spawning ffmpeg.
pub async fn start_transcode(
    session: Arc<RwLock<TranscodeSession>>,
    params: TranscodeParams,
) -> Result<()> {
    {
        let mut s = session.write().await;
        s.state = TranscodeState::Running;
        let _ = s.state_tx.send(TranscodeState::Running);
    }

    let session_clone = session.clone();

    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&params.output_dir)?;

        let args = build_hls_args(&params);
        debug!("ffmpeg args: {:?}", args);

        let output = std::process::Command::new(ffmpeg_bin())
            .args(&args)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| anyhow!("Failed to spawn ffmpeg: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "ffmpeg exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        info!("HLS transcode job completed");
        Ok(())
    });

    match handle.await {
        Ok(Ok(())) => {
            let mut s = session_clone.write().await;
            s.state = TranscodeState::Complete;
            let _ = s.state_tx.send(TranscodeState::Complete);
            info!(session_id = %s.id, "Transcode completed successfully");
        }
        Ok(Err(e)) => {
            let mut s = session_clone.write().await;
            let err_msg = format!("{:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode failed");
            s.state = TranscodeState::Error(err_msg.clone());
            let _ = s.state_tx.send(TranscodeState::Error(err_msg));
        }
        Err(e) => {
            let mut s = session_clone.write().await;
            let err_msg = format!("Task panicked: {:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode task panicked");
            s.state = TranscodeState::Error(err_msg.clone());
            let _ = s.state_tx.send(TranscodeState::Error(err_msg));
        }
    }

    Ok(())
}

/// Parameters for a progressive (non-HLS) transcode that streams to stdout.
#[derive(Debug, Clone)]
pub struct ProgressiveTranscodeParams {
    pub input_url: String,
    pub container: String,   // "mp4", "ts", "mkv", "webm"
    pub video_codec: String, // "copy", "libx264", "libx265", "libvpx-vp9"
    pub audio_codec: String, // "copy", "aac", "libopus"
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

/// Build the ffmpeg CLI args for a progressive transcode piped to stdout.
fn build_progressive_args(params: &ProgressiveTranscodeParams) -> Vec<String> {
    let ffmpeg_video_codec = match params.video_codec.as_str() {
        "copy" => "copy",
        "libx264" | "h264" => "libx264",
        "libx265" | "hevc" => "libx265",
        "libvpx-vp9" | "vp9" => "libvpx-vp9",
        other => other,
    };
    let ffmpeg_audio_codec = match params.audio_codec.as_str() {
        "copy" => "copy",
        "aac" => "aac",
        "libopus" | "opus" => "libopus",
        "mp3" => "libmp3lame",
        other => other,
    };

    // When stream-copying into MP4 we need bitstream filters; promote to MKV instead.
    let format = {
        let requested = match params.container.as_str() {
            "ts" | "mpegts" => "mpegts",
            "webm" => "webm",
            "mkv" | "matroska" => "matroska",
            _ => "mp4",
        };
        if ffmpeg_video_codec == "copy" && requested == "mp4" {
            "matroska"
        } else {
            requested
        }
    };

    let mut args: Vec<String> = vec![
        "-v".into(), "error".into(),
        "-analyzeduration".into(), "5000000".into(),
        "-probesize".into(), "5000000".into(),
        "-reconnect".into(), "1".into(),
        "-reconnect_at_eof".into(), "1".into(),
        "-reconnect_streamed".into(), "1".into(),
        "-reconnect_delay_max".into(), "5".into(),
    ];

    // Input seek (fast, before -i)
    if let Some(ticks) = params.start_time_ticks {
        let secs = ticks as f64 / 10_000_000.0;
        args.extend(["-ss".into(), format!("{:.6}", secs)]);
    }

    args.extend(["-i".into(), params.input_url.clone()]);

    // Stream mapping
    let scale_filter = if ffmpeg_video_codec != "copy" {
        let tp = TranscodeParams {
            max_width: params.max_width,
            max_height: params.max_height,
            ..Default::default()
        };
        build_scale_filter(&tp)
    } else {
        None
    };

    if let Some(ref filter) = scale_filter {
        args.extend(["-vf".into(), filter.clone()]);
    } else if params.audio_stream_index.is_some() || params.subtitle_stream_index.is_some() {
        args.extend(["-map".into(), "0:v".into()]);
        if let Some(audio_idx) = params.audio_stream_index {
            args.extend(["-map".into(), format!("0:{}", audio_idx)]);
        } else {
            args.extend(["-map".into(), "0:a?".into()]);
        }
        if let Some(sub_idx) = params.subtitle_stream_index {
            args.extend(["-map".into(), format!("0:{}?", sub_idx)]);
        }
    }

    // Video
    args.extend(["-c:v".into(), ffmpeg_video_codec.into()]);
    if ffmpeg_video_codec == "copy" {
        // Force hvc1 tag for HEVC Apple compatibility
        args.extend(["-tag:v".into(), "hvc1".into()]);
    } else if let Some(bitrate) = params.video_bitrate {
        args.extend(["-b:v".into(), bitrate.to_string()]);
    } else {
        args.extend(["-preset".into(), "fast".into()]);
    }

    // Audio
    args.extend(["-c:a".into(), ffmpeg_audio_codec.into()]);
    if ffmpeg_audio_codec != "copy" {
        if let Some(bitrate) = params.audio_bitrate {
            args.extend(["-b:a".into(), bitrate.to_string()]);
        }
        if let Some(ch) = params.audio_channels {
            args.extend(["-ac".into(), ch.to_string()]);
        }
    }

    // Format-specific flags
    args.extend(["-strict".into(), "unofficial".into()]);
    if format == "mp4" {
        args.extend([
            "-movflags".into(),
            "frag_keyframe+empty_moov+default_base_moof".into(),
        ]);
    }

    args.extend(["-f".into(), format.into(), "pipe:1".into()]);

    args
}

/// Start a progressive transcode that returns a readable byte stream.
pub fn start_progressive_transcode(
    params: ProgressiveTranscodeParams,
) -> Result<
    impl futures::Stream<Item = std::result::Result<bytes::Bytes, std::io::Error>>,
> {
    let args = build_progressive_args(&params);
    debug!("ffmpeg progressive args: {:?}", args);

    let mut child = tokio::process::Command::new(ffmpeg_bin())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn ffmpeg: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Failed to capture ffmpeg stdout"))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Failed to capture ffmpeg stderr"))?;

    // Log stderr and reap child when done.
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.is_empty() {
                error!("ffmpeg: {}", line);
            }
        }
        match child.wait().await {
            Ok(status) if !status.success() => {
                error!("progressive ffmpeg exited: {}", status)
            }
            Ok(status) => debug!("progressive ffmpeg exited: {}", status),
            Err(e) => error!("progressive ffmpeg wait error: {}", e),
        }
    });

    info!(
        "Starting progressive transcode (container={}, vcodec={}, acodec={})",
        params.container, params.video_codec, params.audio_codec
    );

    Ok(tokio_util::io::ReaderStream::new(stdout))
}

/// Generate a master HLS playlist that references the variant playlist.
pub fn generate_master_playlist(session: &TranscodeSession) -> String {
    let play_session_id = &session.id;

    let video_codec_str = match session.video_codec.as_str() {
        "copy" => "avc1.640028",
        "h264" | "libx264" => "avc1.640028",
        "hevc" | "libx265" => "hvc1.1.6.L150.B0",
        _ => "avc1.640028",
    };
    let audio_codec_str = match session.audio_codec.as_str() {
        "copy" | "aac" => "mp4a.40.2",
        _ => "mp4a.40.2",
    };
    let codecs = format!("{},{}", video_codec_str, audio_codec_str);

    format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-STREAM-INF:BANDWIDTH=2000000,AVERAGE-BANDWIDTH=2000000,CODECS=\"{}\"\n\
         main.m3u8?PlaySessionId={}\n",
        codecs, play_session_id
    )
}
