use crate::jellyfin;
use anyhow::{Result, anyhow};
use ez_ffmpeg::stream_info::{probe_media_info, StreamInfo};
use isolang::Language;
use std::str::FromStr;

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

/// Probe a media URL and return a Jellyfin MediaSourceInfo directly.
pub fn probe_media(url: &str) -> Result<jellyfin::MediaSourceInfo> {
    tracing::debug!(url, "probing media");

    let info = ez_ffmpeg::stream_info::probe_media_info(url)
        .map_err(|e| anyhow!("Failed to probe media {}: {}", url, e))?;

    // Get duration (in microseconds) and convert to ticks (100ns units)
    let run_time_ticks = nonzero(info.duration_us).map(|us| us * 10); // 1 µs = 10 ticks

    // Get container format
    let container = {
        let f = info.format_name;
        // FFmpeg returns compound format names like "matroska,webm" — take the first
        let base = f.split(',').next().unwrap_or(&f).to_string();
        match base.as_str() {
            "matroska" => Some("mkv".to_string()),
            "mov" => Some("mp4".to_string()),
            "mpegts" => Some("ts".to_string()),
            other => Some(other.to_string()),
        }
    };

    tracing::debug!(?run_time_ticks, ?container, "probe container info");

    let all_streams = info.streams;

    let mut streams: Vec<jellyfin::MediaStream> = Vec::new();
    let mut overall_bitrate: Option<i64> = None;

    // Track per-type indices (Jellyfin uses per-type indices for audio/subtitle selection)
    let mut _video_idx: i64 = 0;
    let mut _audio_idx: i64 = 0;
    let mut _sub_idx: i64 = 0;

    for info in &all_streams {
        match info {
            StreamInfo::Video {
                index,
                codec_name,
                width,
                height,
                bit_rate,
                fps,
                metadata,
                ..
            } => {
                let bitrate = nonzero(*bit_rate);
                if overall_bitrate.is_none() {
                    overall_bitrate = bitrate;
                }

                let language = metadata.get("language").map(|s: &String| s.as_str());
                let title = metadata.get("title").cloned();

                let fps_f32 = if *fps > 0.0 { Some(*fps as f32) } else { None };

                streams.push(jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Video),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    width: nonzero(*width as i64),
                    height: nonzero(*height as i64),
                    bit_rate: bitrate,
                    average_frame_rate: fps_f32.and_then(nonzero),
                    real_frame_rate: fps_f32.and_then(nonzero),
                    is_default: Some(_video_idx == 0),
                    is_forced: Some(false),
                    display_title: display_title(
                        language,
                        Some(codec_name),
                        "Video",
                        None,
                    ),
                    language: language.map(|s: &str| s.to_string()),
                    title,
                    ..Default::default()
                });
                _video_idx += 1;
            }
            StreamInfo::Audio {
                index,
                codec_name,
                sample_rate,
                nb_channels,
                bit_rate,
                metadata,
                ..
            } => {
                let bitrate = nonzero(*bit_rate);
                let channels = nonzero(*nb_channels);

                let language = metadata.get("language").map(|s: &String| s.as_str());
                let title = metadata.get("title").cloned();

                streams.push(jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Audio),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    channels: channels.map(|v| v as i64),
                    sample_rate: nonzero(*sample_rate as i64),
                    bit_rate: bitrate,
                    is_default: Some(_audio_idx == 0),
                    is_forced: Some(false),
                    display_title: display_title(
                        language,
                        Some(codec_name),
                        "Audio",
                        channels,
                    ),
                    language: language.map(|s: &str| s.to_string()),
                    title,
                    ..Default::default()
                });
                _audio_idx += 1;
            }
            StreamInfo::Subtitle {
                index,
                codec_name,
                metadata,
                ..
            } => {
                let language = metadata.get("language").map(|s: &String| s.as_str());
                let title = metadata.get("title").cloned();

                streams.push(jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Subtitle),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    is_default: Some(_sub_idx == 0),
                    is_forced: Some(false),
                    display_title: display_title(
                        language,
                        Some(codec_name),
                        "Subtitle",
                        None,
                    ),
                    language: language.map(|s: &str| s.to_string()),
                    title,
                    ..Default::default()
                });
                _sub_idx += 1;
            }
            _ => {} // Skip Data, Attachment, Unknown streams
        }
    }

    // Set default stream indices (matches Jellyfin behavior)
    let default_audio_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(jellyfin::MediaStreamType::Audio)))
        .and_then(|s| s.index);
    let default_subtitle_stream_index = streams
        .iter()
        .find(|s| matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle)))
        .and_then(|s| s.index);

    Ok(jellyfin::MediaSourceInfo {
        media_streams: streams,
        container,
        run_time_ticks,
        bitrate: overall_bitrate,
        default_audio_stream_index,
        default_subtitle_stream_index,
        ..Default::default()
    })
}
