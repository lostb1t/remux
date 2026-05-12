use anyhow::{Result, anyhow};
#[cfg(unix)]
use libc;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use remux_sdks::remux::HardwareAccelerationType;

use super::session::{TranscodeSession, TranscodeState};

pub async fn detect_hardware_acceleration() -> HardwareAccelerationType {
    let detected = probe_hw_accel().await;
    info!(hw_accel = ?detected, "Hardware acceleration detected");
    detected
}

/// Detect the VAAPI driver name for the primary DRM render node.
/// Returns "iHD" for Intel (vendor 0x8086), empty string for others.
pub fn detect_vaapi_driver() -> String {
    let vendor = std::fs::read_to_string("/sys/class/drm/renderD128/device/vendor")
        .ok()
        .map(|s| s.trim().to_string());
    match vendor.as_deref() {
        Some("0x8086") => "iHD".to_string(),
        _ => String::new(),
    }
}

async fn probe_hw_accel() -> HardwareAccelerationType {
    let supported = match tokio::process::Command::new(ffmpeg_bin())
        .args(["-hide_banner", "-hwaccels"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(e) => {
            warn!("Could not run ffmpeg to detect hwaccels: {e}");
            String::new()
        }
    };

    select_hw_accel(
        &supported,
        |p| std::path::Path::new(p).exists(),
        || {
            std::fs::read_to_string("/sys/class/drm/renderD128/device/vendor")
                .ok()
                .map(|s| s.trim().to_string())
        },
    )
}

pub(crate) fn select_hw_accel(
    hwaccels_output: &str,
    device_exists: impl Fn(&str) -> bool,
    drm_vendor: impl Fn() -> Option<String>,
) -> HardwareAccelerationType {
    let has = |name: &str| hwaccels_output.lines().any(|l| l.trim() == name);

    let has_render_node = device_exists("/dev/dri/renderD128");
    let is_intel = drm_vendor().as_deref() == Some("0x8086");

    if has("cuda") && device_exists("/dev/nvidia0") {
        HardwareAccelerationType::Nvenc
    } else if has("qsv") && has_render_node && is_intel {
        HardwareAccelerationType::Qsv
    } else if has("vaapi") && has_render_node {
        HardwareAccelerationType::Vaapi
    } else if has("videotoolbox") && cfg!(target_os = "macos") {
        HardwareAccelerationType::VideoToolbox
    } else if has("v4l2m2m") && device_exists("/dev/video0") {
        HardwareAccelerationType::V4l2m2m
    } else if has("rkmpp") && device_exists("/dev/mpp_service") {
        HardwareAccelerationType::Rkmpp
    } else {
        HardwareAccelerationType::None
    }
}

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
    pub burn_subtitle: bool,
    /// Native dimensions of the subtitle bitmap (PGS canvas size), used to
    /// scale the subtitle to match the output video resolution.
    pub subtitle_width: Option<u32>,
    pub subtitle_height: Option<u32>,
    pub encoding_preset: Option<String>,
    /// Codec of the source video stream (e.g. "hevc", "h264"), used to apply
    /// codec-specific output flags such as `-tag:v hvc1` for HEVC in HLS.
    pub source_video_codec: Option<String>,
    pub hardware_acceleration_type: HardwareAccelerationType,
    /// VAAPI render device path.
    pub vaapi_device: String,
    /// VAAPI driver name (e.g. "iHD" for Intel). Empty string means auto-detect.
    pub vaapi_driver: String,
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
            burn_subtitle: false,
            subtitle_width: None,
            subtitle_height: None,
            encoding_preset: None,
            source_video_codec: None,
            hardware_acceleration_type: HardwareAccelerationType::None,
            vaapi_device: "/dev/dri/renderD128".to_string(),
            vaapi_driver: String::new(),
        }
    }
}

/// Return the expected output video dimensions based on transcode params.
fn output_dimensions(params: &TranscodeParams) -> (Option<u32>, Option<u32>) {
    (params.max_width, params.max_height)
}

/// Return the ffmpeg input args that enable hardware-accelerated decoding.
/// These are placed **before** the `-i` flag.
fn hw_input_args(
    accel: HardwareAccelerationType,
    vaapi_device: &str,
    vaapi_driver: &str,
) -> Vec<String> {
    // Build the vaapi init_hw_device string, appending ",driver=X" when known.
    let vaapi_init = |alias: &str| {
        let driver_opt = if vaapi_driver.is_empty() {
            String::new()
        } else {
            format!(",driver={vaapi_driver}")
        };
        format!("vaapi={alias}:{vaapi_device}{driver_opt}")
    };

    match accel {
        HardwareAccelerationType::Nvenc => vec!["-hwaccel".into(), "cuda".into()],
        HardwareAccelerationType::Vaapi => {
            // Initialise the VAAPI device via init_hw_device so that the
            // driver= option takes effect (fixes iHD resolution on Intel).
            // Frames are decoded in software; hwupload in the filter chain
            // uploads the scaled frame to the VAAPI device for encoding.
            vec![
                "-init_hw_device".into(),
                vaapi_init("va"),
                "-filter_hw_device".into(),
                "va".into(),
            ]
        }
        HardwareAccelerationType::Qsv => {
            // On Linux, QSV is derived from a VAAPI device.  We initialise the
            // VAAPI device first (with an explicit driver so iHD is found on
            // Intel), derive a QSV device from it, then use VAAPI for hardware-
            // assisted decoding.  Frames stay in GPU memory; scale_vaapi +
            // hwmap map them to a QSV surface for the QSV encoder.
            vec![
                "-init_hw_device".into(),
                vaapi_init("va"),
                "-init_hw_device".into(),
                "qsv=qs@va".into(),
                "-filter_hw_device".into(),
                "qs".into(),
                "-hwaccel".into(),
                "vaapi".into(),
                "-hwaccel_output_format".into(),
                "vaapi".into(),
            ]
        }
        HardwareAccelerationType::VideoToolbox => {
            vec!["-hwaccel".into(), "videotoolbox".into()]
        }
        HardwareAccelerationType::Amf => vec!["-hwaccel".into(), "d3d11va".into()],
        HardwareAccelerationType::Rkmpp => vec!["-hwaccel".into(), "rkmpp".into()],
        // v4l2m2m and None: software decode
        _ => vec![],
    }
}

/// Map a software encoder name to the equivalent hardware encoder name.
/// Returns the original name unchanged if the codec should not be re-mapped
/// (e.g. "copy", "libvpx-vp9") or if `accel` is None.
fn hw_encoder_name(base: &str, accel: HardwareAccelerationType) -> String {
    let suffix = match accel {
        HardwareAccelerationType::Nvenc => "_nvenc",
        HardwareAccelerationType::Vaapi => "_vaapi",
        HardwareAccelerationType::Qsv => "_qsv",
        HardwareAccelerationType::Amf => "_amf",
        HardwareAccelerationType::VideoToolbox => "_videotoolbox",
        HardwareAccelerationType::V4l2m2m => "_v4l2m2m",
        HardwareAccelerationType::Rkmpp => "_rkmpp",
        _ => return base.to_string(),
    };
    match base {
        "libx264" => format!("h264{suffix}"),
        "libx265" => format!("hevc{suffix}"),
        other => other.to_string(),
    }
}

/// Append any filter steps required by the hardware encoder after the
/// scale/video filter chain (e.g. hwupload for VAAPI).
fn hw_filter_suffix(accel: HardwareAccelerationType) -> Option<String> {
    match accel {
        // VAAPI encoder requires frames in VAAPI memory; upload them after scaling.
        HardwareAccelerationType::Vaapi => Some("format=nv12,hwupload".to_string()),
        // QSV: frames are in VAAPI memory after decode; map them to a QSV surface
        // for the QSV encoder.  scale_vaapi (the scale step) already converted the
        // pixel format, so we only need the device remap here.
        HardwareAccelerationType::Qsv => {
            Some("hwmap=derive_device=qsv,format=qsv".to_string())
        }
        _ => None,
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

/// Build a VAAPI hardware scale filter for QSV transcoding.
///
/// When using QSV (VAAPI-decode → QSV-encode pipeline) frames live in VAAPI
/// GPU memory, so we must use `scale_vaapi` instead of the CPU `scale` filter.
/// Always returns `Some` — at minimum a format-conversion pass is needed so the
/// `hwmap=derive_device=qsv` suffix can map frames into QSV memory.
/// `extra_hw_frames=24` follows Jellyfin's recommendation for VAAPI VPP pools.
fn build_qsv_scale_filter(max_width: Option<u32>, max_height: Option<u32>) -> String {
    match (max_width, max_height) {
        (Some(w), Some(h)) => format!(
            "scale_vaapi=w='min({w},iw)':h='min({h},ih)':format=nv12:extra_hw_frames=24:force_original_aspect_ratio=decrease"
        ),
        (Some(w), None) => {
            format!("scale_vaapi=w='min({w},iw)':h=-2:format=nv12:extra_hw_frames=24")
        }
        (None, Some(h)) => {
            format!("scale_vaapi=w=-2:h='min({h},ih)':format=nv12:extra_hw_frames=24")
        }
        _ => "scale_vaapi=format=nv12:extra_hw_frames=24".to_string(),
    }
}

/// Build the ffmpeg CLI args for an HLS transcode.
pub(crate) fn build_hls_args(params: &TranscodeParams) -> Vec<String> {
    let accel = params.hardware_acceleration_type;
    let is_hw = !matches!(accel, HardwareAccelerationType::None);

    let ffmpeg_video_codec = {
        let base = match params.video_codec.as_str() {
            "copy" => "copy",
            "h264" | "libx264" => "libx264",
            "hevc" | "libx265" | "h265" => "libx265",
            other => other,
        };
        // Subtitle burn-in requires re-encoding; can't copy video.
        let base = if params.burn_subtitle
            && params.subtitle_stream_index.is_some()
            && base == "copy"
        {
            "libx264"
        } else {
            base
        };
        if base != "copy" && is_hw {
            hw_encoder_name(base, accel)
        } else {
            base.to_string()
        }
    };
    let ffmpeg_audio_codec = match params.audio_codec.as_str() {
        "copy" => "copy",
        _ => "aac",
    };

    // fMP4 (fragmented MP4) is required for HEVC on iOS Safari per Apple's HLS
    // authoring specification.  MPEG-TS cannot carry HEVC correctly in HLS.
    let is_hevc_copy = ffmpeg_video_codec == "copy"
        && matches!(
            params.source_video_codec.as_deref(),
            Some("hevc") | Some("h265") | Some("hvc1") | Some("hev1")
        );

    let mut args: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-analyzeduration".into(),
        "1000000".into(),
        "-probesize".into(),
        "1000000".into(),
    ];

    // Hardware acceleration input flags (before -ss and -i)
    args.extend(hw_input_args(
        accel,
        &params.vaapi_device,
        &params.vaapi_driver,
    ));

    // Input seek (fast, before -i)
    if let Some(ticks) = params.start_time_ticks {
        let secs = ticks as f64 / 10_000_000.0;
        args.extend(["-ss".into(), format!("{:.6}", secs)]);
    }

    args.extend([
        "-copyts".into(),
        "-i".into(),
        params.input_url.clone(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-max_muxing_queue_size".into(),
        "2048".into(),
    ]);

    let hw_suffix = hw_filter_suffix(accel);

    // Stream mapping
    if params.burn_subtitle {
        if let Some(sub_idx) = params.subtitle_stream_index {
            // Scale subtitle bitmap to output dimensions (matching Jellyfin's approach).
            // When output size is known, scale to that; otherwise pass through as-is.
            let (out_w, out_h) = output_dimensions(params);
            let sub_scale = match (out_w, out_h) {
                (Some(w), Some(h)) => format!("scale={w}:{h}:fast_bilinear"),
                (Some(w), None) => format!("scale={w}:-1:fast_bilinear"),
                (None, Some(h)) => format!("scale=-1:{h}:fast_bilinear"),
                _ => String::new(),
            };

            // PGS bitmaps are BGRA; bare `scale` converts to a pixel format
            // compatible with overlay before any resize step (mirrors Jellyfin).
            let sub_preproc = if sub_scale.is_empty() {
                "scale".to_string()
            } else {
                format!("scale,{sub_scale}")
            };
            let main_scale_part = build_scale_filter(params)
                .map(|s| format!("{s}"))
                .unwrap_or_default();
            let overlay = "[main][sub]overlay=eof_action=pass:repeatlast=0";
            let filter = if main_scale_part.is_empty() {
                let base =
                    format!("[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{overlay}[v]");
                match &hw_suffix {
                    Some(suf) => format!(
                        "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{overlay}[vraw];[vraw]{suf}[v]"
                    ),
                    None => base,
                }
            } else {
                match &hw_suffix {
                    Some(suf) => format!(
                        "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{main_scale_part}[main];[main][sub]{overlay}[vraw];[vraw]{suf}[v]"
                    ),
                    None => format!(
                        "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{main_scale_part}[main];[main][sub]{overlay}[v]"
                    ),
                }
            };
            args.extend(["-filter_complex".into(), filter]);
            args.extend(["-map".into(), "[v]".into()]);
            if let Some(audio_idx) = params.audio_stream_index {
                args.extend(["-map".into(), format!("0:{}", audio_idx)]);
            } else {
                args.extend(["-map".into(), "0:a?".into()]);
            }
        }
    } else {
        let scale_filter = if ffmpeg_video_codec != "copy" {
            if matches!(accel, HardwareAccelerationType::Qsv) {
                // QSV pipeline: frames in VAAPI memory — must use scale_vaapi.
                // Always Some so the hwmap suffix is always combined with it.
                Some(build_qsv_scale_filter(params.max_width, params.max_height))
            } else {
                build_scale_filter(params)
            }
        } else {
            None
        };
        let vf = match (&scale_filter, &hw_suffix) {
            (Some(s), Some(suf)) => Some(format!("{s},{suf}")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(suf)) if ffmpeg_video_codec != "copy" => Some(suf.clone()),
            _ => None,
        };
        if let Some(ref filter) = vf {
            args.extend(["-vf".into(), filter.clone()]);
        } else if let Some(audio_idx) = params.audio_stream_index {
            args.extend([
                "-map".into(),
                "0:v:0".into(),
                "-map".into(),
                format!("0:{}", audio_idx),
            ]);
        }
    }

    // Video codec
    args.extend(["-c:v".into(), ffmpeg_video_codec.clone()]);

    if ffmpeg_video_codec == "copy" {
        if is_hevc_copy {
            args.extend(["-tag:v".into(), "hvc1".into()]);
            // fMP4 stores HEVC in HVCC format natively — no hevc_mp4toannexb needed.
            // Strip embedded Dolby Vision RPU NALs so VideoToolbox treats this as
            // plain HDR10 rather than Dolby Vision (avoids black video on some devices).
            args.extend(["-bsf:v".into(), "dovi_rpu=strip=1".into()]);
        }
    } else if is_hw {
        // HW encoders use bitrate control; CRF/preset/profile flags don't apply.
        if let Some(bitrate) = params.video_bitrate {
            args.extend(["-b:v".into(), bitrate.to_string()]);
        }
    } else if ffmpeg_video_codec == "libx264" {
        let preset = params.encoding_preset.as_deref().unwrap_or("fast");
        args.extend([
            "-profile:v".into(),
            "high".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-crf".into(),
            "23".into(),
            "-preset".into(),
            preset.to_string(),
            "-tune".into(),
            "zerolatency".into(),
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
    let seg_ext = if is_hevc_copy { "m4s" } else { "ts" };
    let segment = params.output_dir.join(format!("segment_%05d.{}", seg_ext));

    let start_number = params
        .start_time_ticks
        .map(|t| {
            (t as f64 / 10_000_000.0 / params.segment_length as f64).floor() as u32
        })
        .unwrap_or(0);

    args.extend([
        "-f".into(),
        "hls".into(),
        "-hls_time".into(),
        params.segment_length.to_string(),
        "-start_number".into(),
        start_number.to_string(),
        "-hls_segment_filename".into(),
        segment.to_string_lossy().into_owned(),
        "-hls_playlist_type".into(),
        "event".into(),
        "-hls_list_size".into(),
        "0".into(),
    ]);

    if is_hevc_copy {
        // Apple HLS spec: HEVC must be delivered in fMP4/CMAF segments, not MPEG-TS.
        // Use just the filename for init; ffmpeg places it alongside the segments.
        args.extend([
            "-hls_segment_type".into(),
            "fmp4".into(),
            "-hls_fmp4_init_filename".into(),
            "init.mp4".into(),
        ]);
    }

    args.push(playlist.to_string_lossy().into_owned());

    args
}

/// Spawn an ffmpeg process, drain its stderr to DEBUG logs, and wait for it
/// to finish (or be killed via `kill_rx`).  Returns the exit result and the
/// accumulated stderr text for error reporting.
async fn run_ffmpeg(
    args: Vec<String>,
    kill_rx: tokio::sync::oneshot::Receiver<()>,
    monitor_stop_tx: tokio::sync::oneshot::Sender<()>,
    ffmpeg_pid_out: &mut u32,
) -> (
    Option<std::result::Result<std::process::ExitStatus, std::io::Error>>,
    String,
) {
    let child = tokio::process::Command::new(ffmpeg_bin())
        .args(&args)
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = monitor_stop_tx.send(());
            return (Some(Err(e)), String::new());
        }
    };

    *ffmpeg_pid_out = child.id().unwrap_or(0);
    let stderr = child.stderr.take();

    let (stderr_tx, stderr_rx) = tokio::sync::oneshot::channel::<String>();
    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            let mut buf = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.is_empty() {
                    debug!("ffmpeg: {}", line);
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            let _ = stderr_tx.send(buf);
        });
    } else {
        let _ = stderr_tx.send(String::new());
    }

    let pid = *ffmpeg_pid_out;
    let result = tokio::select! {
        r = child.wait() => Some(r),
        _ = kill_rx => {
            #[cfg(unix)]
            send_signal(pid, libc::SIGCONT);
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    };

    let _ = monitor_stop_tx.send(());
    let stderr_out = stderr_rx.await.unwrap_or_default();
    (result, stderr_out)
}

/// Start an HLS transcode job by spawning ffmpeg.
/// If hardware-accelerated encoding fails, automatically retries once with
/// software encoding so a broken HW driver doesn't permanently block playback.
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

    let session_clone = session.clone();
    tokio::spawn(async move {
        let mut params = params;
        let mut sw_fallback = false;

        loop {
            let args = build_hls_args(&params);
            debug!("ffmpeg args: {}", args.join(" "));

            let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
            let (monitor_stop_tx, monitor_stop_rx) =
                tokio::sync::oneshot::channel::<()>();

            let mut ffmpeg_pid: u32 = 0;
            {
                let mut s = session_clone.write().await;
                s.start_time_secs = params
                    .start_time_ticks
                    .map(|t| (t / 10_000_000) as u32)
                    .unwrap_or(0);
                s.kill_tx = Some(kill_tx);
                spawn_buffer_monitor(
                    s.output_dir.clone(),
                    s.segment_length,
                    s.playback_offset_secs.clone(),
                    0, // updated below once child spawns
                    monitor_stop_rx,
                );
            }

            let (result, stderr_out) =
                run_ffmpeg(args, kill_rx, monitor_stop_tx, &mut ffmpeg_pid).await;

            let using_hw = !matches!(
                params.hardware_acceleration_type,
                HardwareAccelerationType::None
            );

            // If HW accel caused the failure, retry once with software encoding.
            if !sw_fallback
                && using_hw
                && matches!(&result, Some(Ok(s)) if !s.success())
            {
                warn!(
                    accel = ?params.hardware_acceleration_type,
                    stderr = stderr_out.trim(),
                    "HW-accelerated transcode failed — retrying with software encoding"
                );
                // Clean partial output so the retry starts fresh.
                let _ = std::fs::remove_dir_all(&params.output_dir);
                let _ = std::fs::create_dir_all(&params.output_dir);
                params.hardware_acceleration_type = HardwareAccelerationType::None;
                sw_fallback = true;
                continue;
            }

            let mut s = session_clone.write().await;
            s.kill_tx = None;

            match result {
                Some(Ok(status)) if status.success() => {
                    s.state = TranscodeState::Complete;
                    let _ = s.state_tx.send(TranscodeState::Complete);
                    info!(session_id = %s.id, sw_fallback, "Transcode completed successfully");
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
                    debug!(session_id = %s.id, "ffmpeg killed by session stop");
                }
            }

            s.wait_done.notify_one();
            break;
        }
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
    pub burn_subtitle: bool,
    pub subtitle_width: Option<u32>,
    pub subtitle_height: Option<u32>,
    pub encoding_preset: Option<String>,
    pub source_video_codec: Option<String>,
    pub hardware_acceleration_type: HardwareAccelerationType,
    pub vaapi_device: String,
    /// VAAPI driver name (e.g. "iHD" for Intel). Empty string means auto-detect.
    pub vaapi_driver: String,
}

/// Build the ffmpeg CLI args for a progressive transcode piped to stdout.
pub(crate) fn build_progressive_args(
    params: &ProgressiveTranscodeParams,
) -> Vec<String> {
    let accel = params.hardware_acceleration_type;
    let is_hw = !matches!(accel, HardwareAccelerationType::None);

    let ffmpeg_video_codec = {
        let base = match params.video_codec.as_str() {
            "copy" => "copy",
            "libx264" | "h264" => "libx264",
            "libx265" | "hevc" => "libx265",
            "libvpx-vp9" | "vp9" => "libvpx-vp9",
            other => other,
        };
        let base = if params.burn_subtitle
            && params.subtitle_stream_index.is_some()
            && base == "copy"
        {
            "libx264"
        } else {
            base
        };
        if base != "copy" && is_hw {
            hw_encoder_name(base, accel)
        } else {
            base.to_string()
        }
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

    // Hardware acceleration input flags (before -ss and -i)
    args.extend(hw_input_args(
        accel,
        &params.vaapi_device,
        &params.vaapi_driver,
    ));

    // Input seek (fast, before -i)
    if let Some(ticks) = params.start_time_ticks {
        let secs = ticks as f64 / 10_000_000.0;
        args.extend(["-ss".into(), format!("{:.6}", secs)]);
    }

    args.extend(["-i".into(), params.input_url.clone()]);

    let hw_suffix = hw_filter_suffix(accel);

    // Stream mapping
    let scale_filter = if ffmpeg_video_codec != "copy" {
        if matches!(accel, HardwareAccelerationType::Qsv) {
            Some(build_qsv_scale_filter(params.max_width, params.max_height))
        } else {
            let tp = TranscodeParams {
                max_width: params.max_width,
                max_height: params.max_height,
                ..Default::default()
            };
            build_scale_filter(&tp)
        }
    } else {
        None
    };

    if params.burn_subtitle {
        if let Some(sub_idx) = params.subtitle_stream_index {
            let (out_w, out_h) = (params.max_width, params.max_height);
            let sub_scale = match (out_w, out_h) {
                (Some(w), Some(h)) => format!("scale={w}:{h}:fast_bilinear"),
                (Some(w), None) => format!("scale={w}:-1:fast_bilinear"),
                (None, Some(h)) => format!("scale=-1:{h}:fast_bilinear"),
                _ => String::new(),
            };
            let sub_preproc = if sub_scale.is_empty() {
                "scale".to_string()
            } else {
                format!("scale,{sub_scale}")
            };
            let overlay = "[main][sub]overlay=eof_action=pass:repeatlast=0";
            let filter = match (&scale_filter, &hw_suffix) {
                (Some(main_scale), Some(suf)) => format!(
                    "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{main_scale}[main];[main][sub]{overlay}[vraw];[vraw]{suf}[v]"
                ),
                (Some(main_scale), None) => format!(
                    "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0]{main_scale}[main];[main][sub]{overlay}[v]"
                ),
                (None, Some(suf)) => format!(
                    "[0:{sub_idx}]{sub_preproc}[sub];[0:v:0][sub]{overlay}[vraw];[vraw]{suf}[v]"
                ),
                (None, None) => {
                    format!("[0:{sub_idx}]{sub_preproc}[sub];[0:v:0][sub]{overlay}[v]")
                }
            };
            args.extend(["-filter_complex".into(), filter]);
            args.extend(["-map".into(), "[v]".into()]);
            if let Some(audio_idx) = params.audio_stream_index {
                args.extend(["-map".into(), format!("0:{}", audio_idx)]);
            } else {
                args.extend(["-map".into(), "0:a?".into()]);
            }
        }
    } else {
        let vf = match (&scale_filter, &hw_suffix) {
            (Some(s), Some(suf)) => Some(format!("{s},{suf}")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(suf)) if ffmpeg_video_codec != "copy" => Some(suf.clone()),
            _ => None,
        };
        if let Some(ref filter) = vf {
            args.extend(["-vf".into(), filter.clone()]);
        } else if params.audio_stream_index.is_some()
            || params.subtitle_stream_index.is_some()
        {
            args.extend(["-map".into(), "0:v:0".into()]);
            if let Some(audio_idx) = params.audio_stream_index {
                args.extend(["-map".into(), format!("0:{}", audio_idx)]);
            } else {
                args.extend(["-map".into(), "0:a?".into()]);
            }
            if let Some(sub_idx) = params.subtitle_stream_index {
                args.extend(["-map".into(), format!("0:{}?", sub_idx)]);
            }
        }
    }

    // Video
    args.extend(["-c:v".into(), ffmpeg_video_codec.clone()]);
    if ffmpeg_video_codec == "copy" {
        // Apply hvc1 codec tag for HEVC Apple compatibility
        if matches!(
            params.source_video_codec.as_deref(),
            Some("hevc") | Some("h265") | Some("hvc1") | Some("hev1")
        ) {
            args.extend(["-tag:v".into(), "hvc1".into()]);
        }
    } else if is_hw {
        // HW encoders use bitrate control; CRF/preset/profile flags don't apply.
        if let Some(bitrate) = params.video_bitrate {
            args.extend(["-b:v".into(), bitrate.to_string()]);
        }
    } else {
        if ffmpeg_video_codec == "libx264" {
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
        }
        if let Some(bitrate) = params.video_bitrate {
            args.extend(["-b:v".into(), bitrate.to_string()]);
        } else {
            let preset = params.encoding_preset.as_deref().unwrap_or("fast");
            args.extend(["-preset".into(), preset.to_string()]);
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

    // Log stderr line-by-line at DEBUG and reap child when done.
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.is_empty() {
                debug!("ffmpeg: {}", line);
            }
        }
        match child.wait().await {
            Ok(status) if !status.success() => {
                if status.code() == Some(224) {
                    debug!(
                        "progressive ffmpeg exited after client disconnect: {}",
                        status
                    )
                } else {
                    error!("progressive ffmpeg exited: {}", status)
                }
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
pub fn generate_variant_playlist(
    session: &TranscodeSession,
    query_string: &str,
) -> String {
    let runtime_ticks = session.runtime_ticks;
    let segment_length = session.segment_length;
    let play_session_id = &session.id;
    let use_fmp4 = session.use_fmp4();
    // fMP4 segments require HLS version 7; standard TS segments need version 6.
    let version = if use_fmp4 { 7u32 } else { 6u32 };
    let seg_ext = if use_fmp4 { "m4s" } else { "ts" };

    let extra_qs = if query_string.is_empty() {
        String::new()
    } else {
        format!("&{}", query_string)
    };

    if runtime_ticks <= 0 || segment_length == 0 {
        // Fallback: single long segment (shouldn't happen in practice)
        let mut buf = format!(
            "#EXTM3U\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n\
             #EXT-X-VERSION:{version}\n\
             #EXT-X-TARGETDURATION:{seg}\n\
             #EXT-X-MEDIA-SEQUENCE:0\n",
            version = version,
            seg = segment_length,
        );
        if use_fmp4 {
            buf.push_str(&format!(
                "#EXT-X-MAP:URI=\"init.mp4?PlaySessionId={}\"\n",
                play_session_id
            ));
        }
        buf.push_str(&format!(
            "#EXTINF:{seg}.000000, nodesc\n\
             segment_00000.{ext}?PlaySessionId={psid}{qs}\n\
             #EXT-X-ENDLIST\n",
            seg = segment_length,
            ext = seg_ext,
            psid = play_session_id,
            qs = extra_qs,
        ));
        return buf;
    }

    let seg_length_ticks = segment_length as i64 * 10_000_000;
    let whole_segments = runtime_ticks / seg_length_ticks;
    let remaining_ticks = runtime_ticks % seg_length_ticks;
    let total_segments = whole_segments + if remaining_ticks > 0 { 1 } else { 0 };

    let target_duration = segment_length; // always an integer ceiling

    let mut buf = String::with_capacity(total_segments as usize * 120);
    buf.push_str("#EXTM3U\n");
    buf.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    buf.push_str(&format!("#EXT-X-VERSION:{}\n", version));
    buf.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", target_duration));
    buf.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    if use_fmp4 {
        buf.push_str(&format!(
            "#EXT-X-MAP:URI=\"init.mp4?PlaySessionId={}\"\n",
            play_session_id
        ));
    }

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
            "segment_{:05}.{}?PlaySessionId={}&runtimeTicks={}&actualSegmentLengthTicks={}{}\n",
            i,
            seg_ext,
            play_session_id,
            cumulative_ticks,
            length_ticks,
            extra_qs,
        ));

        cumulative_ticks += length_ticks;
    }

    buf.push_str("#EXT-X-ENDLIST\n");
    buf
}

/// Generate the HEVC codec string for HLS CODECS attribute.
/// Matches Jellyfin's `HlsCodecStringHelpers.GetH265String()`.
fn hevc_hls_codec_string(profile: Option<&str>, level: Option<f64>) -> String {
    let profile_part = match profile {
        Some(p)
            if p.eq_ignore_ascii_case("main 10")
                || p.eq_ignore_ascii_case("main10") =>
        {
            "2.4"
        }
        _ => "1.4",
    };
    let level_val = level.unwrap_or(150.0) as i32;
    format!("hvc1.{}.L{}.B0", profile_part, level_val)
}

/// Generate a master HLS playlist that references the variant playlist.
pub fn generate_master_playlist(session: &TranscodeSession) -> String {
    use remux_sdks::remux::VideoRangeType;

    let play_session_id = &session.id;

    let video_codec_str: String = match session.video_codec.as_str() {
        "copy" => match session.source_video_codec.as_deref() {
            Some("hevc") | Some("h265") | Some("hvc1") | Some("hev1") => {
                hevc_hls_codec_string(
                    session.source_video_profile.as_deref(),
                    session.source_video_level,
                )
            }
            _ => "avc1.640028".to_string(),
        },
        "h264" | "libx264" => "avc1.640028".to_string(),
        "hevc" | "libx265" => hevc_hls_codec_string(
            session.source_video_profile.as_deref(),
            session.source_video_level,
        ),
        _ => "avc1.640028".to_string(),
    };
    let audio_codec_str = match session.audio_codec.as_str() {
        "copy" | "aac" => "mp4a.40.2",
        _ => "mp4a.40.2",
    };
    let codecs = format!("{},{}", video_codec_str, audio_codec_str);

    // VIDEO-RANGE: only meaningful for copy-mode (passthrough) video. Transcoded output is SDR.
    let video_range_attr = if session.video_codec == "copy" {
        match session.source_video_range_type {
            Some(VideoRangeType::Hdr10)
            | Some(VideoRangeType::Hdr10Plus)
            | Some(VideoRangeType::DoviWithHdr10)
            | Some(VideoRangeType::Dovi) => ",VIDEO-RANGE=PQ",
            Some(VideoRangeType::Hlg) | Some(VideoRangeType::DoviWithHlg) => {
                ",VIDEO-RANGE=HLG"
            }
            _ => ",VIDEO-RANGE=SDR",
        }
    } else {
        ",VIDEO-RANGE=SDR"
    };

    let resolution_attr =
        match (session.source_video_width, session.source_video_height) {
            (Some(w), Some(h)) => format!(",RESOLUTION={}x{}", w, h),
            _ => String::new(),
        };

    let frame_rate_attr = match session.source_frame_rate {
        Some(fps) if fps > 0.0 => format!(",FRAME-RATE={:.3}", fps),
        _ => String::new(),
    };

    tracing::debug!(
        source_video_codec = ?session.source_video_codec,
        source_video_profile = ?session.source_video_profile,
        source_video_level = ?session.source_video_level,
        source_video_range_type = ?session.source_video_range_type,
        source_video_width = session.source_video_width,
        source_video_height = session.source_video_height,
        source_frame_rate = session.source_frame_rate,
        video_codec = session.video_codec.as_str(),
        codecs = codecs.as_str(),
        "generating master HLS playlist"
    );

    format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:6\n\
         #EXT-X-INDEPENDENT-SEGMENTS\n\
         #EXT-X-STREAM-INF:BANDWIDTH=2000000,AVERAGE-BANDWIDTH=2000000,CODECS=\"{codecs}\"{video_range}{resolution}{frame_rate}\n\
         main.m3u8?PlaySessionId={play_session_id}\n",
        codecs = codecs,
        video_range = video_range_attr,
        resolution = resolution_attr,
        frame_rate = frame_rate_attr,
        play_session_id = play_session_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn args_contains(args: &[String], flag: &str) -> bool {
        args.iter().any(|a| a == flag)
    }

    fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].as_str())
    }

    // ── select_hw_accel tests ─────────────────────────────────────────────────

    fn no_devices(_: &str) -> bool {
        false
    }
    fn all_devices(_: &str) -> bool {
        true
    }

    #[test]
    fn hw_none_when_no_hwaccels_listed() {
        assert_eq!(
            select_hw_accel("", all_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_none_when_devices_absent() {
        let hwaccels =
            "Hardware acceleration methods:\ncuda\nvaapi\nqsv\nv4l2m2m\nrkmpp\n";
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_nvenc_requires_cuda_and_device() {
        let hwaccels = "Hardware acceleration methods:\ncuda\n";
        // Device present
        assert_eq!(
            select_hw_accel(hwaccels, |p| p == "/dev/nvidia0", || None),
            HardwareAccelerationType::Nvenc
        );
        // Device absent
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_vaapi_requires_vaapi_and_device() {
        let hwaccels = "Hardware acceleration methods:\nvaapi\n";
        assert_eq!(
            select_hw_accel(hwaccels, |p| p == "/dev/dri/renderD128", || None),
            HardwareAccelerationType::Vaapi
        );
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_qsv_beats_vaapi_when_both_available() {
        let hwaccels = "Hardware acceleration methods:\nvaapi\nqsv\n";
        assert_eq!(
            select_hw_accel(
                hwaccels,
                |p| p == "/dev/dri/renderD128",
                || { Some("0x8086".to_string()) }
            ),
            HardwareAccelerationType::Qsv
        );
    }

    #[test]
    fn hw_qsv_requires_dri_device() {
        let hwaccels = "Hardware acceleration methods:\nqsv\n";
        assert_eq!(
            select_hw_accel(
                hwaccels,
                |p| p == "/dev/dri/renderD128",
                || { Some("0x8086".to_string()) }
            ),
            HardwareAccelerationType::Qsv
        );
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || Some("0x8086".to_string())),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_rkmpp_requires_mpp_service_device() {
        let hwaccels = "Hardware acceleration methods:\nrkmpp\n";
        assert_eq!(
            select_hw_accel(hwaccels, |p| p == "/dev/mpp_service", || None),
            HardwareAccelerationType::Rkmpp
        );
        // This was the Asahi Linux bug: rkmpp listed but device absent → None
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_v4l2m2m_requires_video0_device() {
        let hwaccels = "Hardware acceleration methods:\nv4l2m2m\n";
        assert_eq!(
            select_hw_accel(hwaccels, |p| p == "/dev/video0", || None),
            HardwareAccelerationType::V4l2m2m
        );
        assert_eq!(
            select_hw_accel(hwaccels, no_devices, || None),
            HardwareAccelerationType::None
        );
    }

    #[test]
    fn hw_videotoolbox_only_on_macos() {
        let hwaccels = "Hardware acceleration methods:\nvideotoolbox\n";
        let expected = if cfg!(target_os = "macos") {
            HardwareAccelerationType::VideoToolbox
        } else {
            HardwareAccelerationType::None
        };
        assert_eq!(select_hw_accel(hwaccels, all_devices, || None), expected);
    }

    #[test]
    fn hw_priority_nvenc_over_vaapi() {
        // When both are available nvenc wins
        let hwaccels = "Hardware acceleration methods:\ncuda\nvaapi\n";
        assert_eq!(
            select_hw_accel(hwaccels, all_devices, || None),
            HardwareAccelerationType::Nvenc
        );
    }

    #[test]
    fn hw_falls_through_to_rkmpp_when_others_absent() {
        let hwaccels =
            "Hardware acceleration methods:\ncuda\nvaapi\nqsv\nv4l2m2m\nrkmpp\n";
        // Only /dev/mpp_service present
        assert_eq!(
            select_hw_accel(hwaccels, |p| p == "/dev/mpp_service", || None),
            HardwareAccelerationType::Rkmpp
        );
    }

    fn default_hls(output_dir: PathBuf) -> TranscodeParams {
        TranscodeParams {
            input_url: "http://localhost/test.mkv".into(),
            output_dir,
            ..Default::default()
        }
    }

    fn default_progressive() -> ProgressiveTranscodeParams {
        ProgressiveTranscodeParams {
            input_url: "http://localhost/test.mkv".into(),
            container: "mp4".into(),
            video_codec: "copy".into(),
            audio_codec: "aac".into(),
            start_time_ticks: None,
            max_width: None,
            max_height: None,
            video_bitrate: None,
            audio_bitrate: None,
            audio_channels: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
            burn_subtitle: false,
            subtitle_width: None,
            subtitle_height: None,
            encoding_preset: None,
            source_video_codec: None,
            hardware_acceleration_type: HardwareAccelerationType::None,
            vaapi_device: "/dev/dri/renderD128".into(),
            vaapi_driver: String::new(),
        }
    }

    // ── HLS tests ────────────────────────────────────────────────────────────

    #[test]
    fn hls_basic_copy() {
        let dir = PathBuf::from("/tmp/test_session");
        let args = build_hls_args(&default_hls(dir.clone()));

        assert_eq!(arg_after(&args, "-c:v"), Some("copy"));
        assert_eq!(arg_after(&args, "-c:a"), Some("aac"));
        assert_eq!(arg_after(&args, "-f"), Some("hls"));
        // Default TS segments — no fmp4 flag
        assert!(!args_contains(&args, "-hls_segment_type"));
        // Playlist and segment paths
        assert!(args.iter().any(|a| a.ends_with("main.m3u8")));
        assert!(
            args.iter()
                .any(|a| a.contains("segment_") && a.ends_with(".ts"))
        );
    }

    #[test]
    fn hls_hevc_copy_uses_fmp4() {
        let dir = PathBuf::from("/tmp/test_hevc");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "copy".into(),
            source_video_codec: Some("hevc".into()),
            ..default_hls(dir)
        });

        assert_eq!(arg_after(&args, "-hls_segment_type"), Some("fmp4"));
        assert_eq!(
            arg_after(&args, "-hls_fmp4_init_filename"),
            Some("init.mp4")
        );
        assert_eq!(arg_after(&args, "-tag:v"), Some("hvc1"));
        // Dolby Vision strip bsf
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-bsf:v" && w[1].contains("dovi_rpu"))
        );
        // Segments use .m4s extension
        assert!(
            args.iter()
                .any(|a| a.contains("segment_") && a.ends_with(".m4s"))
        );
    }

    #[test]
    fn hls_hevc_copy_hvc1_tag_alias() {
        // "hvc1" codec string should also trigger fMP4 path
        let dir = PathBuf::from("/tmp/test_hvc1");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "copy".into(),
            source_video_codec: Some("hvc1".into()),
            ..default_hls(dir)
        });
        assert_eq!(arg_after(&args, "-hls_segment_type"), Some("fmp4"));
        assert_eq!(arg_after(&args, "-tag:v"), Some("hvc1"));
    }

    #[test]
    fn hls_libx264_transcode_flags() {
        let dir = PathBuf::from("/tmp/test_x264");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            ..default_hls(dir)
        });

        assert_eq!(arg_after(&args, "-c:v"), Some("libx264"));
        assert_eq!(arg_after(&args, "-crf"), Some("23"));
        assert_eq!(arg_after(&args, "-preset"), Some("fast"));
        assert_eq!(arg_after(&args, "-profile:v"), Some("high"));
        assert_eq!(arg_after(&args, "-tune"), Some("zerolatency"));
        assert_eq!(arg_after(&args, "-pix_fmt"), Some("yuv420p"));
    }

    #[test]
    fn hls_libx264_custom_preset_and_bitrate() {
        let dir = PathBuf::from("/tmp/test_x264_br");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            encoding_preset: Some("veryfast".into()),
            video_bitrate: Some(4_000_000),
            ..default_hls(dir)
        });

        assert_eq!(arg_after(&args, "-preset"), Some("veryfast"));
        assert_eq!(arg_after(&args, "-maxrate"), Some("4000000"));
        assert_eq!(arg_after(&args, "-bufsize"), Some("8000000"));
    }

    #[test]
    fn hls_seek_offset_placed_before_input() {
        let dir = PathBuf::from("/tmp/test_seek");
        let ticks: i64 = 30 * 10_000_000; // 30 seconds
        let args = build_hls_args(&TranscodeParams {
            start_time_ticks: Some(ticks),
            ..default_hls(dir)
        });

        let ss_pos = args.iter().position(|a| a == "-ss").expect("-ss missing");
        let i_pos = args.iter().position(|a| a == "-i").expect("-i missing");
        assert!(ss_pos < i_pos, "-ss must come before -i");
        assert_eq!(args[ss_pos + 1], "30.000000");

        // start_number = floor(30 / 6) = 5
        assert_eq!(arg_after(&args, "-start_number"), Some("5"));
    }

    #[test]
    fn hls_no_seek_start_number_zero() {
        let dir = PathBuf::from("/tmp/test_noseek");
        let args = build_hls_args(&default_hls(dir));
        assert_eq!(arg_after(&args, "-start_number"), Some("0"));
        assert!(!args_contains(&args, "-ss"));
    }

    #[test]
    fn hls_scale_filter_both_dimensions() {
        let dir = PathBuf::from("/tmp/test_scale");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            max_width: Some(1920),
            max_height: Some(1080),
            ..default_hls(dir)
        });

        let vf = arg_after(&args, "-vf").expect("-vf missing");
        assert!(vf.contains("min(1920,iw)"), "vf: {vf}");
        assert!(vf.contains("min(1080,ih)"), "vf: {vf}");
        assert!(
            vf.contains("force_original_aspect_ratio=decrease"),
            "vf: {vf}"
        );
    }

    #[test]
    fn hls_scale_filter_width_only() {
        let dir = PathBuf::from("/tmp/test_scale_w");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            max_width: Some(1280),
            ..default_hls(dir)
        });
        let vf = arg_after(&args, "-vf").expect("-vf missing");
        assert!(vf.contains("min(1280,iw)"), "vf: {vf}");
        assert!(vf.contains(":-2"), "vf: {vf}");
    }

    #[test]
    fn hls_audio_bitrate_and_channels() {
        let dir = PathBuf::from("/tmp/test_audio");
        let args = build_hls_args(&TranscodeParams {
            audio_bitrate: Some(192_000),
            audio_channels: Some(2),
            ..default_hls(dir)
        });

        assert_eq!(arg_after(&args, "-b:a"), Some("192000"));
        assert_eq!(arg_after(&args, "-ac"), Some("2"));
    }

    #[test]
    fn hls_audio_copy_no_bitrate_flags() {
        let dir = PathBuf::from("/tmp/test_acopy");
        let args = build_hls_args(&TranscodeParams {
            audio_codec: "copy".into(),
            ..default_hls(dir)
        });
        assert_eq!(arg_after(&args, "-c:a"), Some("copy"));
        assert!(
            !args_contains(&args, "-b:a"),
            "must not set -b:a when copying audio"
        );
        assert!(
            !args_contains(&args, "-ac"),
            "must not set -ac when copying audio"
        );
    }

    #[test]
    fn hls_subtitle_burn_forces_reencode_and_filter_complex() {
        let dir = PathBuf::from("/tmp/test_sub");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "copy".into(),
            burn_subtitle: true,
            subtitle_stream_index: Some(2),
            ..default_hls(dir)
        });

        // copy → libx264 forced by subtitle burn
        assert_eq!(arg_after(&args, "-c:v"), Some("libx264"));
        // filter_complex with overlay
        let fc = arg_after(&args, "-filter_complex").expect("-filter_complex missing");
        assert!(fc.contains("overlay"), "filter_complex: {fc}");
        assert!(
            fc.contains("[0:2]"),
            "filter_complex should reference sub stream: {fc}"
        );
        // map [v] output label
        assert!(args.windows(2).any(|w| w[0] == "-map" && w[1] == "[v]"));
    }

    #[test]
    fn hls_subtitle_burn_with_scale_in_filter_complex() {
        let dir = PathBuf::from("/tmp/test_sub_scale");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            burn_subtitle: true,
            subtitle_stream_index: Some(3),
            max_width: Some(1280),
            max_height: Some(720),
            ..default_hls(dir)
        });

        let fc = arg_after(&args, "-filter_complex").expect("-filter_complex missing");
        assert!(
            fc.contains("scale=1280:720:fast_bilinear"),
            "sub scale: {fc}"
        );
        assert!(fc.contains("min(1280,iw)"), "video scale: {fc}");
        assert!(fc.contains("overlay"), "overlay: {fc}");
    }

    #[test]
    fn hls_nvenc_hardware_accel() {
        let dir = PathBuf::from("/tmp/test_nvenc");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            hardware_acceleration_type: HardwareAccelerationType::Nvenc,
            ..default_hls(dir)
        });

        // Input flag before -i
        let hwaccel_pos = args
            .iter()
            .position(|a| a == "-hwaccel")
            .expect("-hwaccel missing");
        let i_pos = args.iter().position(|a| a == "-i").expect("-i missing");
        assert!(hwaccel_pos < i_pos);
        assert_eq!(args[hwaccel_pos + 1], "cuda");
        // Encoder remapped
        assert_eq!(arg_after(&args, "-c:v"), Some("h264_nvenc"));
    }

    #[test]
    fn hls_vaapi_hardware_accel() {
        let dir = PathBuf::from("/tmp/test_vaapi");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            hardware_acceleration_type: HardwareAccelerationType::Vaapi,
            vaapi_device: "/dev/dri/renderD128".into(),
            vaapi_driver: "iHD".into(),
            ..default_hls(dir)
        });

        // init_hw_device with driver= instead of legacy -vaapi_device
        assert!(
            args.windows(2).any(|w| w[0] == "-init_hw_device"
                && w[1].contains("vaapi=va:")
                && w[1].contains("/dev/dri/renderD128")
                && w[1].contains("driver=iHD")),
            "expected init_hw_device vaapi with iHD driver, got: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-filter_hw_device" && w[1] == "va"),
            "expected -filter_hw_device va"
        );
        // legacy -vaapi_device must NOT appear
        assert!(!args.windows(2).any(|w| w[0] == "-vaapi_device"));
        // Encoder remapped
        assert_eq!(arg_after(&args, "-c:v"), Some("h264_vaapi"));
        // hwupload suffix in -vf
        let vf = arg_after(&args, "-vf").expect("-vf missing for VAAPI");
        assert!(vf.contains("format=nv12,hwupload"), "vf: {vf}");
    }

    #[test]
    fn hls_vaapi_no_driver_omits_driver_option() {
        let dir = PathBuf::from("/tmp/test_vaapi_amd");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx264".into(),
            hardware_acceleration_type: HardwareAccelerationType::Vaapi,
            vaapi_device: "/dev/dri/renderD128".into(),
            vaapi_driver: String::new(), // AMD / unknown: no driver=
            ..default_hls(dir)
        });

        let init_dev = args
            .windows(2)
            .find(|w| w[0] == "-init_hw_device")
            .expect("init_hw_device missing");
        assert!(
            !init_dev[1].contains("driver="),
            "unexpected driver= in {}",
            init_dev[1]
        );
        assert!(init_dev[1].contains("vaapi=va:"));
    }

    #[test]
    fn hls_qsv_hardware_accel() {
        let dir = PathBuf::from("/tmp/test_qsv");
        let args = build_hls_args(&TranscodeParams {
            video_codec: "libx265".into(),
            hardware_acceleration_type: HardwareAccelerationType::Qsv,
            vaapi_device: "/dev/dri/renderD128".into(),
            vaapi_driver: "iHD".into(),
            ..default_hls(dir)
        });

        // VAAPI device init with iHD driver
        assert!(
            args.windows(2).any(|w| w[0] == "-init_hw_device"
                && w[1].starts_with("vaapi=va:")
                && w[1].contains("driver=iHD")),
            "expected VAAPI init with iHD: {args:?}"
        );
        // QSV device derived from VAAPI
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-init_hw_device" && w[1] == "qsv=qs@va"),
            "expected qsv=qs@va init"
        );
        // filter_hw_device points at QSV
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-filter_hw_device" && w[1] == "qs"),
            "expected -filter_hw_device qs"
        );
        // VAAPI hwaccel (not qsv) for decode
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-hwaccel" && w[1] == "vaapi"),
            "expected -hwaccel vaapi"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-hwaccel_output_format" && w[1] == "vaapi"),
            "expected -hwaccel_output_format vaapi"
        );
        // Encoder remapped to QSV
        assert_eq!(arg_after(&args, "-c:v"), Some("hevc_qsv"));
        // scale_vaapi in -vf
        let vf = arg_after(&args, "-vf").expect("-vf missing for QSV");
        assert!(
            vf.contains("scale_vaapi"),
            "expected scale_vaapi in vf: {vf}"
        );
        assert!(
            vf.contains("hwmap=derive_device=qsv"),
            "expected hwmap in vf: {vf}"
        );
        assert!(vf.contains("format=qsv"), "expected format=qsv in vf: {vf}");
    }

    // ── progressive tests ────────────────────────────────────────────────────

    #[test]
    fn progressive_basic_copy() {
        let args = build_progressive_args(&default_progressive());

        assert_eq!(arg_after(&args, "-c:v"), Some("copy"));
        assert_eq!(arg_after(&args, "-c:a"), Some("aac"));
        // Copy into mp4 is promoted to matroska to avoid bsf issues
        assert_eq!(arg_after(&args, "-f"), Some("matroska"));
        assert!(args.last() == Some(&"pipe:1".to_string()));
    }

    #[test]
    fn progressive_mp4_transcode_uses_mp4_format() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            video_codec: "libx264".into(),
            container: "mp4".into(),
            ..default_progressive()
        });
        assert_eq!(arg_after(&args, "-f"), Some("mp4"));
        assert!(args_contains(&args, "-movflags"));
    }

    #[test]
    fn progressive_ts_container() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            container: "ts".into(),
            ..default_progressive()
        });
        assert_eq!(arg_after(&args, "-f"), Some("mpegts"));
    }

    #[test]
    fn progressive_seek_before_input() {
        let ticks: i64 = 60 * 10_000_000; // 60 s
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            start_time_ticks: Some(ticks),
            ..default_progressive()
        });
        let ss_pos = args.iter().position(|a| a == "-ss").expect("-ss missing");
        let i_pos = args.iter().position(|a| a == "-i").expect("-i missing");
        assert!(ss_pos < i_pos);
        assert_eq!(args[ss_pos + 1], "60.000000");
    }

    #[test]
    fn progressive_hevc_copy_adds_hvc1_tag() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            source_video_codec: Some("hevc".into()),
            ..default_progressive()
        });
        assert_eq!(arg_after(&args, "-tag:v"), Some("hvc1"));
    }

    #[test]
    fn progressive_nvenc_remaps_encoder() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            video_codec: "libx264".into(),
            container: "mp4".into(),
            hardware_acceleration_type: HardwareAccelerationType::Nvenc,
            ..default_progressive()
        });
        assert_eq!(arg_after(&args, "-c:v"), Some("h264_nvenc"));
    }

    #[test]
    fn progressive_subtitle_burn_filter_complex() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            video_codec: "copy".into(),
            burn_subtitle: true,
            subtitle_stream_index: Some(1),
            ..default_progressive()
        });
        // copy → libx264 forced
        assert_eq!(arg_after(&args, "-c:v"), Some("libx264"));
        let fc = arg_after(&args, "-filter_complex").expect("-filter_complex missing");
        assert!(fc.contains("overlay"), "fc: {fc}");
    }

    #[test]
    fn progressive_audio_channels_and_bitrate() {
        let args = build_progressive_args(&ProgressiveTranscodeParams {
            audio_codec: "aac".into(),
            audio_bitrate: Some(256_000),
            audio_channels: Some(6),
            ..default_progressive()
        });
        assert_eq!(arg_after(&args, "-b:a"), Some("256000"));
        assert_eq!(arg_after(&args, "-ac"), Some("6"));
    }

    #[test]
    fn progressive_reconnect_flags_present() {
        let args = build_progressive_args(&default_progressive());
        assert!(args_contains(&args, "-reconnect"));
        assert!(args_contains(&args, "-reconnect_at_eof"));
        assert!(args_contains(&args, "-reconnect_streamed"));
    }
}
