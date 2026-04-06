use anyhow::{Result, anyhow};
#[cfg(unix)]
use libc;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::session::{TranscodeSession, TranscodeState};

/// Max seconds to buffer ahead of the current playback position.
const MAX_BUFFER_SECS: u32 = 300;
/// Seconds behind the playback position before a segment is eligible for deletion.
const SEGMENT_KEEP_SECS: u32 = 300;

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}

/// Send SIGSTOP/SIGCONT to a process by PID on Unix.
#[cfg(unix)]
fn send_signal(pid: u32, sig: libc::c_int) {
    unsafe { libc::kill(pid as libc::pid_t, sig) };
}
#[cfg(not(unix))]
fn send_signal(_pid: u32, _sig: i32) {}

/// Spawn the buffer-throttle task. It pauses/resumes ffmpeg so it never
/// encodes more than MAX_BUFFER_SECS ahead of what the client has requested.
fn spawn_buffer_monitor(
    output_dir: PathBuf,
    segment_length: u32,
    playback_offset_secs: Arc<std::sync::atomic::AtomicU32>,
    ffmpeg_pid: u32,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let mut paused = false;
        let mut ticks: u32 = 0;
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
            ticks += 1;

            let produced = count_segments(&output_dir);
            let buffered_secs = produced * segment_length;
            // playback_offset_secs is how far the client has actually played
            // relative to the start of this transcode session (from progress reports).
            let playback_secs = playback_offset_secs.load(Ordering::Relaxed);

            let ahead = buffered_secs.saturating_sub(playback_secs);

            if !paused && ahead >= MAX_BUFFER_SECS {
                debug!(pid = ffmpeg_pid, ahead, "Buffer full — pausing ffmpeg");
                #[cfg(unix)]
                send_signal(ffmpeg_pid, libc::SIGSTOP);
                paused = true;
            } else if paused
                && ahead < MAX_BUFFER_SECS.saturating_sub(segment_length * 2)
            {
                debug!(pid = ffmpeg_pid, ahead, "Buffer drained — resuming ffmpeg");
                #[cfg(unix)]
                send_signal(ffmpeg_pid, libc::SIGCONT);
                paused = false;
            }

            // Every 30 seconds, delete segments that are more than SEGMENT_KEEP_SECS
            // behind the current playback position.
            if ticks % 30 == 0 && playback_secs > SEGMENT_KEEP_SECS {
                let cutoff_idx = (playback_secs - SEGMENT_KEEP_SECS) / segment_length;
                delete_old_segments(&output_dir, cutoff_idx);
            }
        }

        // Ensure ffmpeg isn't left paused when we stop monitoring.
        if paused {
            #[cfg(unix)]
            send_signal(ffmpeg_pid, libc::SIGCONT);
        }
    });
}

/// Delete `.ts` segment files whose index is less than `cutoff_idx`.
fn delete_old_segments(dir: &PathBuf, cutoff_idx: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".ts") {
            continue;
        }
        // segment_00042.ts → parse the numeric suffix
        let Some(idx_str) = name.strip_suffix(".ts").and_then(|s| s.rsplit('_').next())
        else {
            continue;
        };
        let Ok(idx) = idx_str.parse::<u32>() else {
            continue;
        };
        if idx < cutoff_idx {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn count_segments(dir: &PathBuf) -> u32 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".ts"))
                .count() as u32
        })
        .unwrap_or(0)
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
        "copy" => "copy",
        "h264" | "libx264" => "libx264",
        "hevc" | "libx265" | "h265" => "libx265",
        other => other,
    };
    let ffmpeg_audio_codec = match params.audio_codec.as_str() {
        "copy" => "copy",
        _ => "aac",
    };

    let mut args: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-analyzeduration".into(),
        "1000000".into(),
        "-probesize".into(),
        "1000000".into(),
    ];

    // Input seek (fast, before -i)
    if let Some(ticks) = params.start_time_ticks {
        let secs = ticks as f64 / 10_000_000.0;
        args.extend(["-ss".into(), format!("{:.6}", secs)]);
    }

    args.extend([
        "-copyts".into(),
        "-i".into(), params.input_url.clone(),
        "-avoid_negative_ts".into(), "disabled".into(),
        "-max_muxing_queue_size".into(), "2048".into(),
    ]);

    // Stream mapping
    let scale_filter = if ffmpeg_video_codec != "copy" {
        build_scale_filter(params)
    } else {
        None
    };

    if let Some(ref filter) = scale_filter {
        args.extend(["-vf".into(), filter.clone()]);
    } else if let Some(audio_idx) = params.audio_stream_index {
        args.extend([
            "-map".into(),
            "0:v".into(),
            "-map".into(),
            format!("0:{}", audio_idx),
        ]);
    }

    // Video codec
    args.extend(["-c:v".into(), ffmpeg_video_codec.into()]);

    if ffmpeg_video_codec == "libx264" {
        args.extend([
            "-profile:v".into(), "high".into(),
            "-pix_fmt".into(), "yuv420p".into(),
            "-crf".into(), "23".into(),
            "-preset".into(), "fast".into(),
            "-tune".into(), "zerolatency".into(),
        ]);
        // Use client's max bitrate as a ceiling, not a CBR target.
        // This keeps libx264 memory usage low while honouring the cap.
        if let Some(bitrate) = params.video_bitrate {
            args.extend([
                "-maxrate".into(),
                bitrate.to_string(),
                "-bufsize".into(),
                (bitrate * 2).to_string(),
            ]);
        }
    } else if let Some(bitrate) = params.video_bitrate {
        args.extend(["-b:v".into(), bitrate.to_string()]);
    }

    // Audio codec
    args.extend(["-c:a".into(), ffmpeg_audio_codec.into()]);
    if ffmpeg_audio_codec != "copy" {
        let audio_bitrate = params.audio_bitrate.unwrap_or(128_000);
        args.extend(["-b:a".into(), audio_bitrate.to_string()]);
        if let Some(ch) = params.audio_channels {
            args.extend(["-ac".into(), ch.to_string()]);
        }
    }

    // HLS output
    let playlist = params.output_dir.join("main.m3u8");
    let segment = params.output_dir.join("segment_%05d.ts");
    
    let start_number = params.start_time_ticks
        .map(|t| (t as f64 / 10_000_000.0 / params.segment_length as f64).floor() as u32)
        .unwrap_or(0);

    args.extend([
        "-f".into(), "hls".into(),
        "-hls_time".into(), params.segment_length.to_string(),
        "-start_number".into(), start_number.to_string(),
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

    std::fs::create_dir_all(&params.output_dir)
        .map_err(|e| anyhow!("Failed to create output dir: {}", e))?;

    let args = build_hls_args(&params);
    debug!("ffmpeg args: {:?}", args);

    let mut child = tokio::process::Command::new(ffmpeg_bin())
        .args(&args)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn ffmpeg: {}", e))?;

    let stderr = child.stderr.take();
    let ffmpeg_pid = child.id().unwrap_or(0);

    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    let (monitor_stop_tx, monitor_stop_rx) = tokio::sync::oneshot::channel::<()>();

    {
        let mut s = session.write().await;
        s.start_time_secs = params
            .start_time_ticks
            .map(|t| (t / 10_000_000) as u32)
            .unwrap_or(0);
        spawn_buffer_monitor(
            s.output_dir.clone(),
            s.segment_length,
            s.playback_offset_secs.clone(),
            ffmpeg_pid,
            monitor_stop_rx,
        );
    }

    {
        let mut s = session.write().await;
        s.kill_tx = Some(kill_tx);
    }

    let session_clone = session.clone();
    tokio::spawn(async move {
        // Drain stderr in the background while waiting for exit.
        let stderr_task = async {
            if let Some(stderr) = stderr {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let _ = tokio::io::BufReader::new(stderr)
                    .read_to_string(&mut buf)
                    .await;
                buf
            } else {
                String::new()
            }
        };

        let result = tokio::select! {
            r = child.wait() => Some(r),
            // kill_rx fires when stop() sends (), or when the sender is dropped.
            _ = kill_rx => {
                // Make sure ffmpeg isn't paused before we kill it.
                #[cfg(unix)]
                send_signal(ffmpeg_pid, libc::SIGCONT);
                let _ = child.kill().await;
                let _ = child.wait().await;
                None
            }
        };

        // Stop the buffer monitor.
        let _ = monitor_stop_tx.send(());

        let stderr_out = stderr_task.await;

        let mut s = session_clone.write().await;
        s.kill_tx = None;

        match result {
            Some(Ok(status)) if status.success() => {
                s.state = TranscodeState::Complete;
                let _ = s.state_tx.send(TranscodeState::Complete);
                info!(session_id = %s.id, "Transcode completed successfully");
            }
            Some(Ok(status)) => {
                let err_msg = format!(
                    "ffmpeg exited with status {}: {}",
                    status,
                    stderr_out.trim()
                );
                error!(session_id = %s.id, error = %err_msg, "Transcode failed");
                s.state = TranscodeState::Error(err_msg.clone());
                let _ = s.state_tx.send(TranscodeState::Error(err_msg));
            }
            Some(Err(e)) => {
                let err_msg = format!("Failed to wait for ffmpeg: {}", e);
                error!(session_id = %s.id, error = %err_msg, "Transcode error");
                s.state = TranscodeState::Error(err_msg.clone());
                let _ = s.state_tx.send(TranscodeState::Error(err_msg));
            }
            None => {
                // Killed by stop() — session already removed, no state update needed.
                debug!(session_id = %s.id, "ffmpeg killed by session stop");
            }
        }

        s.wait_done.notify_one();
    });

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
        "-v".into(),
        "error".into(),
        "-analyzeduration".into(),
        "5000000".into(),
        "-probesize".into(),
        "5000000".into(),
        "-reconnect".into(),
        "1".into(),
        "-reconnect_at_eof".into(),
        "1".into(),
        "-reconnect_streamed".into(),
        "1".into(),
        "-reconnect_delay_max".into(),
        "5".into(),
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
    } else if params.audio_stream_index.is_some()
        || params.subtitle_stream_index.is_some()
    {
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
    } else {
        if ffmpeg_video_codec == "libx264" {
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
        }
        if let Some(bitrate) = params.video_bitrate {
            args.extend(["-b:v".into(), bitrate.to_string()]);
        } else {
            args.extend(["-preset".into(), "fast".into()]);
        }
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

/// Generate the variant (child) HLS playlist server-side as a VOD playlist.
///
/// Lists ALL segments from time 0 to the end of the media so HLS.js can seek
/// to any position immediately. Each segment URL includes `runtimeTicks` and
/// `actualSegmentLengthTicks` query params so the segment handler knows the
/// cumulative position when it needs to restart FFmpeg.
pub fn generate_variant_playlist(session: &TranscodeSession, query_string: &str) -> String {
    let runtime_ticks = session.runtime_ticks;
    let segment_length = session.segment_length;
    let play_session_id = &session.id;

    if runtime_ticks <= 0 || segment_length == 0 {
        // Fallback: single long segment (shouldn't happen in practice)
        return format!(
            "#EXTM3U\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n\
             #EXT-X-VERSION:3\n\
             #EXT-X-TARGETDURATION:{seg}\n\
             #EXT-X-MEDIA-SEQUENCE:0\n\
             #EXTINF:{seg}.000000, nodesc\n\
             segment_00000.ts?PlaySessionId={psid}{qs}\n\
             #EXT-X-ENDLIST\n",
            seg = segment_length,
            psid = play_session_id,
            qs = if query_string.is_empty() { String::new() } else { format!("&{}", query_string) },
        );
    }

    let seg_length_ticks = segment_length as i64 * 10_000_000;
    let whole_segments = runtime_ticks / seg_length_ticks;
    let remaining_ticks = runtime_ticks % seg_length_ticks;
    let total_segments = whole_segments + if remaining_ticks > 0 { 1 } else { 0 };

    let target_duration = segment_length; // always an integer ceiling

    let mut buf = String::with_capacity(total_segments as usize * 120);
    buf.push_str("#EXTM3U\n");
    buf.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    buf.push_str("#EXT-X-VERSION:3\n");
    buf.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", target_duration));
    buf.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");

    let mut cumulative_ticks: i64 = 0;
    for i in 0..total_segments {
        let length_ticks = if i < whole_segments {
            seg_length_ticks
        } else {
            remaining_ticks
        };
        let length_secs = length_ticks as f64 / 10_000_000.0;

        buf.push_str(&format!("#EXTINF:{:.6}, nodesc\n", length_secs));
        buf.push_str(&format!(
            "segment_{:05}.ts?PlaySessionId={}&runtimeTicks={}&actualSegmentLengthTicks={}{}\n",
            i,
            play_session_id,
            cumulative_ticks,
            length_ticks,
            if query_string.is_empty() { String::new() } else { format!("&{}", query_string) },
        ));

        cumulative_ticks += length_ticks;
    }

    buf.push_str("#EXT-X-ENDLIST\n");
    buf
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
