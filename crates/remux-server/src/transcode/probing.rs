use crate::api;
use anyhow::{Result, anyhow};
use isolang::Language;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

fn ffprobe_bin() -> String {
    std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".into())
}

fn nonzero<T: Default + PartialOrd>(v: T) -> Option<T> {
    if v > T::default() { Some(v) } else { None }
}

fn display_title(
    language: Option<&str>,
    codec: Option<&str>,
    codec_type: &str,
    channels: Option<i32>,
) -> Option<String> {
    let mut parts: Vec<String> = vec![];

    if let Some(lang) = language.and_then(|code| Language::from_str(code).ok()) {
        parts.push(lang.to_name().to_string());
    }

    if let Some(c) = codec {
        parts.push(c.to_uppercase());
    }

    if codec_type == "Audio" {
        if let Some(ch) = channels {
            let layout = if ch >= 8 {
                "7.1"
            } else if ch >= 6 {
                "5.1"
            } else {
                "Stereo"
            };
            parts.push(layout.to_string());
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" - "))
    }
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Deserialize, Default)]
struct FfprobeDisposition {
    #[serde(default)]
    default: i64,
    #[serde(default)]
    forced: i64,
}

#[derive(Deserialize)]
struct FfprobeStream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    bit_rate: Option<String>,
    avg_frame_rate: Option<String>,
    channels: Option<i64>,
    sample_rate: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: FfprobeDisposition,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    format_name: Option<String>,
    bit_rate: Option<String>,
}

fn parse_frame_rate(s: &str) -> Option<f64> {
    let mut parts = s.splitn(2, '/');
    let num: f64 = parts.next()?.parse().ok()?;
    let den: f64 = parts.next().unwrap_or("1").parse().ok()?;
    if den == 0.0 {
        return None;
    }
    let fps = num / den;
    if fps > 0.0 { Some(fps) } else { None }
}

/// Probe a media URL with ffprobe and return a Jellyfin MediaSourceInfo.
pub fn probe_media(url: &str) -> Result<api::MediaSourceInfo> {
    tracing::debug!(url, "probing media");

    let output = std::process::Command::new(ffprobe_bin())
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            url,
        ])
        .output()
        .map_err(|e| anyhow!("Failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe failed for {}: {}", url, stderr));
    }

    let probe: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow!("Failed to parse ffprobe output: {}", e))?;

    let run_time_ticks = probe
        .format
        .duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|secs| (secs * 1_000_000.0) as i64) // → µs
        .and_then(nonzero)
        .map(|us| us * 10); // µs → 100ns ticks

    let container = probe.format.format_name.as_deref().map(|f| {
        let base = f.split(',').next().unwrap_or(f);
        match base {
            "matroska" => "mkv".to_string(),
            "mov" => "mp4".to_string(),
            "mpegts" => "ts".to_string(),
            other => other.to_string(),
        }
    });

    let overall_bitrate = probe
        .format
        .bit_rate
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(nonzero);

    tracing::debug!(?run_time_ticks, ?container, "probe container info");

    let mut streams: Vec<api::MediaStream> = Vec::new();
    let mut video_idx: i64 = 0;
    let mut audio_idx: i64 = 0;
    let mut sub_idx: i64 = 0;

    for s in &probe.streams {
        let codec_type = s.codec_type.as_deref().unwrap_or("");
        let language = s.tags.get("language").map(|s| s.as_str());
        let title = s.tags.get("title").cloned();

        match codec_type {
            "video" => {
                let bitrate = s
                    .bit_rate
                    .as_deref()
                    .and_then(|b| b.parse::<i64>().ok())
                    .and_then(nonzero);
                let fps = s.avg_frame_rate.as_deref().and_then(parse_frame_rate);
                let codec = s.codec_name.clone().unwrap_or_default();

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Video),
                    index: s.index,
                    codec: Some(codec.clone()),
                    width: s.width.and_then(nonzero),
                    height: s.height.and_then(nonzero),
                    bit_rate: bitrate,
                    average_frame_rate: fps.map(|f| f as f32).and_then(nonzero),
                    real_frame_rate: fps.map(|f| f as f32).and_then(nonzero),
                    is_default: Some(video_idx == 0),
                    is_forced: s.disposition.forced != 0,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    audio_spatial_format: Some("None".to_string()),
                    display_title: display_title(language, Some(&codec), "Video", None),
                    language: language.map(str::to_string),
                    title,
                    ..Default::default()
                });
                video_idx += 1;
            }
            "audio" => {
                let bitrate = s
                    .bit_rate
                    .as_deref()
                    .and_then(|b| b.parse::<i64>().ok())
                    .and_then(nonzero);
                let channels = s.channels.and_then(nonzero);
                let sample_rate = s
                    .sample_rate
                    .as_deref()
                    .and_then(|sr| sr.parse::<i64>().ok())
                    .and_then(nonzero);
                let codec = s.codec_name.clone().unwrap_or_default();

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Audio),
                    index: s.index,
                    codec: Some(codec.clone()),
                    channels,
                    sample_rate,
                    bit_rate: bitrate,
                    is_default: Some(audio_idx == 0),
                    is_forced: s.disposition.forced != 0,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    video_range: Some(api::VideoRange::Unknown),
                    video_range_type: Some(api::VideoRangeType::Unknown),
                    audio_spatial_format: Some("None".to_string()),
                    localized_default: Some("Default".to_string()),
                    localized_external: Some("External".to_string()),
                    display_title: display_title(
                        language,
                        Some(&codec),
                        "Audio",
                        channels.map(|c| c as i32),
                    ),
                    language: language.map(str::to_string),
                    title,
                    ..Default::default()
                });
                audio_idx += 1;
            }
            "subtitle" => {
                let codec = s.codec_name.clone().unwrap_or_default();
                let is_text = matches!(
                    codec.as_str(),
                    "ass" | "ssa" | "subrip" | "webvtt" | "mov_text" | "text"
                );
                let is_image = matches!(
                    codec.as_str(),
                    "pgssub" | "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvdsub"
                );
                let delivery_method = if is_image {
                    Some(api::SubtitleDeliveryMethod::Embed)
                } else {
                    None
                };

                streams.push(api::MediaStream {
                    type_: Some(api::MediaStreamType::Subtitle),
                    index: s.index,
                    codec: Some(codec.clone()),
                    is_default: Some(sub_idx == 0),
                    is_forced: s.disposition.forced != 0,
                    is_avc: Some(false),
                    time_base: Some("1/1000".to_string()),
                    video_range: Some(api::VideoRange::Unknown),
                    video_range_type: Some(api::VideoRangeType::Unknown),
                    audio_spatial_format: Some("None".to_string()),
                    localized_undefined: Some("Undefined".to_string()),
                    localized_default: Some("Default".to_string()),
                    localized_forced: Some("Forced".to_string()),
                    localized_external: Some("External".to_string()),
                    localized_hearing_impaired: Some("Hearing Impaired".to_string()),
                    display_title: display_title(
                        language,
                        Some(&codec),
                        "Subtitle",
                        None,
                    ),
                    language: language.map(str::to_string),
                    title,
                    is_text_subtitle_stream: is_text,
                    supports_external_stream: true,
                    delivery_method,
                    ..Default::default()
                });
                sub_idx += 1;
            }
            _ => {}
        }
    }

    let default_audio_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(api::MediaStreamType::Audio)))
        .map(|s| s.index);
    let default_subtitle_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(api::MediaStreamType::Subtitle)))
        .map(|s| s.index);

    Ok(api::MediaSourceInfo {
        media_streams: streams,
        container,
        run_time_ticks,
        bitrate: overall_bitrate,
        default_audio_stream_index,
        default_subtitle_stream_index,
        ..Default::default()
    })
}
