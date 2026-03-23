use anyhow::{Result, anyhow};
use ez_ffmpeg::{FfmpegContext, Input, Output};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::session::{TranscodeSession, TranscodeState};

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

/// Start an HLS transcode job using ez-ffmpeg.
///
/// This spawns the job on a blocking thread (CPU-bound)
/// and updates the session state accordingly.
pub async fn start_transcode(
    session: Arc<RwLock<TranscodeSession>>,
    params: TranscodeParams,
) -> Result<()> {
    // Update state to Running
    {
        let mut s = session.write().await;
        s.state = TranscodeState::Running;
        let _ = s.state_tx.send(TranscodeState::Running);
    }

    let session_clone = session.clone();
    let params_clone = params.clone();

    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&params_clone.output_dir)?;

        let playlist_path = params_clone.output_dir.join("main.m3u8");
        let segment_pattern = params_clone
            .output_dir
            .join("segment_%05d.ts")
            .to_string_lossy()
            .to_string();

        // Map video codec names to FFmpeg encoder names
        let ffmpeg_video_codec = match params_clone.video_codec.as_str() {
            "copy" | "h264" | "libx264" => "libx264",
            "hevc" | "libx265" | "h265" => "libx265",
            other => other,
        };

        // Map audio codec: respect "copy" when source is already AAC
        let ffmpeg_audio_codec = match params_clone.audio_codec.as_str() {
            "copy" => "copy",
            _ => "aac",
        };

        // Build the output
        let mut output = Output::new(playlist_path.to_string_lossy().to_string())
            .set_format("hls")
            .set_format_opt("hls_time", &params_clone.segment_length.to_string())
            .set_format_opt("hls_segment_filename", &segment_pattern)
            .set_format_opt("hls_playlist_type", "event")
            .set_format_opt("hls_list_size", "0")
            .set_video_codec(ffmpeg_video_codec)
            .set_audio_codec(ffmpeg_audio_codec);

        // Video bitrate
        if let Some(bitrate) = params_clone.video_bitrate {
            output = output.set_video_codec_opt("b", &bitrate.to_string());
        } else if ffmpeg_video_codec == "libx264" {
            output = output
                .set_video_codec_opt("crf", "23")
                .set_video_codec_opt("preset", "fast")
                .set_video_codec_opt("tune", "zerolatency");
        }

        if ffmpeg_video_codec == "libx264" {
            output = output.set_video_codec_opt("profile", "high");
        }

        // Audio bitrate (only when re-encoding)
        if ffmpeg_audio_codec != "copy" {
            if let Some(bitrate) = params_clone.audio_bitrate {
                output = output.set_audio_codec_opt("b", &bitrate.to_string());
            } else {
                output = output.set_audio_codec_opt("b", "128000");
            }
        }

        // Audio channels (only when re-encoding)
        if ffmpeg_audio_codec != "copy" {
            if let Some(channels) = params_clone.audio_channels {
                output = output.set_audio_channels(channels as i32);
            }
        }

        // Build the input with seek on the INPUT side (like Jellyfin's -ss before -i)
        // This enables fast seeking by skipping demuxing before the target position.
        // We set probesize and analyzeduration to small values to prevent FFmpeg from
        // hanging for 30+ seconds on network streams with difficult-to-probe subtitle streams.
        let mut input = Input::from(params_clone.input_url.as_str())
            .set_input_opt("analyzeduration", "1000000")
            .set_input_opt("probesize", "1000000");

        if let Some(ticks) = params_clone.start_time_ticks {
            let start_us = ticks / 10; // 100ns ticks → µs
            input = input.set_start_time_us(start_us);
            output = output.set_start_time_us(start_us);
        }

        let mut builder = FfmpegContext::builder().input(input);

        // Add scale filter if needed (video-only; audio is auto-mapped separately)
        if let Some(filter) = build_scale_filter(&params_clone) {
            builder = builder.filter_desc(filter.as_str());
        }

        let context = builder
            .output(output)
            .build()
            .map_err(|e| anyhow!("Failed to build FFmpeg context: {}", e))?;

        debug!("Starting HLS transcode job via ez-ffmpeg");

        context
            .start()
            .map_err(|e| anyhow!("Failed to start FFmpeg job: {}", e))?
            .wait()
            .map_err(|e| anyhow!("FFmpeg job failed: {}", e))?;

        info!("HLS transcode job completed");

        Ok(())
    });

    // Wait for result and update session state
    match handle.await {
        Ok(Ok(())) => {
            let mut s = session.write().await;
            s.state = TranscodeState::Complete;
            let _ = s.state_tx.send(TranscodeState::Complete);
            info!(session_id = %s.id, "Transcode completed successfully");
        }
        Ok(Err(e)) => {
            let mut s = session.write().await;
            let err_msg = format!("{:#}", e);
            error!(session_id = %s.id, error = %err_msg, "Transcode failed");
            s.state = TranscodeState::Error(err_msg.clone());
            let _ = s.state_tx.send(TranscodeState::Error(err_msg));
        }
        Err(e) => {
            let mut s = session.write().await;
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

/// Start a progressive transcode that returns a readable byte stream.
///
/// Uses ez-ffmpeg's write callback to pipe encoded data to a channel.
pub fn start_progressive_transcode(
    params: ProgressiveTranscodeParams,
) -> Result<
    impl futures::Stream<Item = std::result::Result<bytes::Bytes, std::io::Error>>,
> {
    // Map video codec
    let ffmpeg_video_codec = match params.video_codec.as_str() {
        "copy" => "copy",
        "libx264" | "h264" => "libx264",
        "libx265" | "hevc" => "libx265",
        "libvpx-vp9" | "vp9" => "libvpx-vp9",
        other => other,
    };

    // Map audio codec
    let ffmpeg_audio_codec = match params.audio_codec.as_str() {
        "copy" => "copy",
        "aac" => "aac",
        "libopus" | "opus" => "libopus",
        "mp3" => "libmp3lame",
        other => other,
    };

    // FFmpeg format name
    // IMPORTANT: When video is stream-copied into MP4, FFmpeg requires a bitstream
    // filter (hevc_mp4toannexb / h264_mp4toannexb) to convert the bitstream from the
    // AnnexB format (used in MKV) to the AVCC format (required by MP4).  ez-ffmpeg
    // does not expose a BSF API, so we transparently promote the container to Matroska
    // (MKV), which accepts any bitstream natively.  For re-encoded output, MP4 is fine
    // because the encoder always produces the right packetization.
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

    let (tx, rx) = tokio::sync::mpsc::channel::<
        std::result::Result<bytes::Bytes, std::io::Error>,
    >(32);

    // Track the write position so the seek callback can report it.
    let position = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Build output with write callback
    let pos_write = position.clone();
    let mut output = Output::new_by_write_callback(move |buf: &[u8]| {
        let data = bytes::Bytes::copy_from_slice(buf);
        if tx.blocking_send(Ok(data)).is_err() {
            return -1;
        }
        pos_write.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
        buf.len() as i32
    })
    .set_format(format)
    .set_video_codec(ffmpeg_video_codec)
    .set_audio_codec(ffmpeg_audio_codec);

    // Allow HEVC with Dolby Vision (dvcC/dvvC boxes) and other non-standard
    // but widely-supported features in the output container.
    output = output.set_format_opt("strict", "unofficial");

    // For MP4 streaming, enable fragmented output so no seek is needed.
    // A seek callback is still required by ez-ffmpeg when writing to a custom
    // write callback; provide a position-tracking no-op implementation for both
    // mp4 and matroska (matroska is used when doing copy+mp4 → matroska promotion).
    if format == "mp4" {
        output = output
            .set_format_opt("movflags", "frag_keyframe+empty_moov+default_base_moof");
    }
    if format == "mp4" || format == "matroska" {
        // Force hvc1 tag for HEVC to ensure compatibility with Apple devices and modern players.
        if ffmpeg_video_codec == "copy" {
            output = output.set_video_codec_opt("tag:v", "hvc1");
        }

        let pos_seek = position.clone();
        output = output.set_seek_callback(move |offset: i64, whence: i32| -> i64 {
            match whence {
                0 /* SEEK_SET */ => {
                    pos_seek.store(offset as u64, std::sync::atomic::Ordering::Relaxed);
                    offset
                }
                1 /* SEEK_CUR */ => {
                    let cur = pos_seek.load(std::sync::atomic::Ordering::Relaxed);
                    let new = if offset >= 0 { cur.saturating_add(offset as u64) } else { cur.saturating_sub((-offset) as u64) };
                    pos_seek.store(new, std::sync::atomic::Ordering::Relaxed);
                    new as i64
                }
                2 /* SEEK_END */ => {
                    // For a forward-only stream there is no "end"; report current position.
                    let cur = pos_seek.load(std::sync::atomic::Ordering::Relaxed);
                    cur as i64
                }
                0x10000 /* AVSEEK_SIZE */ => {
                    // Size unknown for a live stream.
                    -1
                }
                _ => -1,
            }
        });
    }

    // Video bitrate
    if ffmpeg_video_codec != "copy" {
        if let Some(bitrate) = params.video_bitrate {
            output = output.set_video_codec_opt("b", &bitrate.to_string());
        } else {
            output = output.set_video_codec_opt("preset", "fast");
        }
    }

    // Audio bitrate
    if ffmpeg_audio_codec != "copy" {
        if let Some(bitrate) = params.audio_bitrate {
            output = output.set_audio_codec_opt("b", &bitrate.to_string());
        }
    }

    // Audio channels
    if let Some(channels) = params.audio_channels {
        output = output.set_audio_channels(channels as i32);
    }

    // Build the input with seek on the INPUT side (fast seek, like Jellyfin's -ss before -i)
    // We set probesize and analyzeduration to safer values to prevent FFmpeg from
    // failing or hanging on network streams with difficult-to-probe streams.
    let mut input = Input::from(params.input_url.as_str())
        .set_input_opt("analyzeduration", "5000000")
        .set_input_opt("probesize", "5000000")
        .set_input_opt("reconnect", "1")
        .set_input_opt("reconnect_at_eof", "1")
        .set_input_opt("reconnect_streamed", "1")
        .set_input_opt("reconnect_delay_max", "5");

    if let Some(ticks) = params.start_time_ticks {
        let start_us = ticks / 10; // 100ns ticks → µs
        input = input.set_start_time_us(start_us);
        // Do NOT set output start time for progressive streams; the player
        // expects the byte stream to start its timestamps from ~0 (or at least
        // consistent within the segment).
    }

    // Build scale filter — only meaningful when video is being re-encoded.
    // Copying video through a scale filter is invalid.
    let use_filter = ffmpeg_video_codec != "copy";
    let scale_filter = if use_filter {
        let scale_params = TranscodeParams {
            max_width: params.max_width,
            max_height: params.max_height,
            ..Default::default()
        };
        build_scale_filter(&scale_params)
    } else {
        None
    };

    // Explicit stream mapping for audio/subtitle selection.
    // Only usable when there is NO filter_desc because ez-ffmpeg's
    // auto-mapping (which binds unlabeled filter outputs) is disabled
    // as soon as any explicit stream map is added.
    if scale_filter.is_none()
        && (params.audio_stream_index.is_some()
            || params.subtitle_stream_index.is_some())
    {
        // Map video
        if ffmpeg_video_codec == "copy" {
            output = output.add_stream_map_with_copy("0:v");
        } else {
            output = output.add_stream_map("0:v");
        }

        // Map specific audio stream or default audio
        if let Some(audio_idx) = params.audio_stream_index {
            if ffmpeg_audio_codec == "copy" {
                output = output.add_stream_map_with_copy(&format!("0:{}", audio_idx));
            } else {
                output = output.add_stream_map(&format!("0:{}", audio_idx));
            }
        } else if ffmpeg_audio_codec == "copy" {
            output = output.add_stream_map_with_copy("0:a?");
        } else {
            output = output.add_stream_map("0:a?");
        }

        // Map specific subtitle stream if requested (always copy subtitles)
        if let Some(sub_idx) = params.subtitle_stream_index {
            output = output.add_stream_map_with_copy(&format!("0:{}?", sub_idx));
        }
    }

    let mut builder = FfmpegContext::builder().input(input);

    if let Some(filter) = scale_filter {
        builder = builder.filter_desc(filter.as_str());
    }

    let context = builder
        .output(output)
        .build()
        .map_err(|e| anyhow!("Failed to create progressive pipeline: {}", e))?;

    info!(
        "Starting progressive transcode (container={}, vcodec={}, acodec={})",
        format, ffmpeg_video_codec, ffmpeg_audio_codec
    );

    // Start the FFmpeg job on a blocking thread
    tokio::task::spawn_blocking(move || match context.start() {
        Ok(handle) => {
            let res: std::result::Result<(), ez_ffmpeg::error::Error> = handle.wait();
            if let Err(e) = res {
                error!("Progressive transcode error: {}", e);
            } else {
                debug!("Progressive transcode completed");
            }
        }
        Err(e) => {
            error!("Failed to start progressive transcode: {}", e);
        }
    });

    Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// Generate a master HLS playlist that references the variant playlist.
/// This mimics Jellyfin's master.m3u8 format.
pub fn generate_master_playlist(session: &TranscodeSession) -> String {
    let play_session_id = &session.id;

    // Build the CODECS string for the STREAM-INF line.
    // hls.js requires this to initialize the correct MSE SourceBuffer type.
    let video_codec_str = match session.video_codec.as_str() {
        "copy" => "avc1.640028", // assume h264 copy; best effort
        "h264" | "libx264" => "avc1.640028", // h264 high profile level 4.0
        "hevc" | "libx265" => "hvc1.1.6.L150.B0",
        _ => "avc1.640028",
    };
    let audio_codec_str = match session.audio_codec.as_str() {
        "copy" | "aac" => "mp4a.40.2", // AAC-LC
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
