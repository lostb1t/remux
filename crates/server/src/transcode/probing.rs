use crate::jellyfin;
use anyhow::{Result, anyhow};
use isolang::Language;
use std::collections::HashMap;
use std::str::FromStr;

fn check_disposition(metadata: &HashMap<String, String>, key: &str) -> bool {
    metadata
        .get(&format!("disposition:{}", key))
        .or_else(|| metadata.get(key))
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
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

/// Map ffmpeg format names to Jellyfin container names.
fn map_container(format: &str) -> String {
    match format {
        "matroska,webm" | "matroska" => "mkv".to_string(),
        "mov,mp4,m4a,3gp,3g2,mj2" => "mp4".to_string(),
        "mpegts" => "ts".to_string(),
        "avi" => "avi".to_string(),
        other => other.split(',').next().unwrap_or(other).to_string(),
    }
}

/// Probe a media URL and return a Jellyfin MediaSourceInfo directly.
pub fn probe_media(url: &str) -> Result<jellyfin::MediaSourceInfo> {
    use crate::ez_ffmpeg::container_info::{get_duration_us, get_format};
    use crate::ez_ffmpeg::stream_info::{StreamInfo, find_all_stream_infos};

    tracing::debug!(url, "probing media");

    let duration_us = get_duration_us(url).ok();
    let format = get_format(url).ok();

    tracing::debug!(?duration_us, ?format, "probe container info");

    let stream_infos = find_all_stream_infos(url)
        .map_err(|e| anyhow!("Failed to probe streams for {}: {}", url, e))?;

    tracing::debug!(
        count = stream_infos.len(),
        "found stream_infos from ez-ffmpeg"
    );

    let mut streams: Vec<jellyfin::MediaStream> = Vec::new();
    let mut bitrate = None;
    for info in stream_infos.iter() {
        let stream = match info {
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
                let language = metadata.get("language").cloned();
                bitrate = nonzero(*bit_rate);
                jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Video),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    width: nonzero(*width).map(|v| v as i64),
                    height: nonzero(*height).map(|v| v as i64),
                    bit_rate: nonzero(*bit_rate),
                    average_frame_rate: nonzero(*fps as f32),
                    real_frame_rate: nonzero(*fps as f32),
                    is_default: Some(check_disposition(metadata, "default")),
                    is_forced: Some(check_disposition(metadata, "forced")),
                    display_title: display_title(
                        language.as_deref(),
                        Some(codec_name),
                        "Video",
                        None,
                    ),
                    language,
                    title: metadata.get("title").cloned(),
                    ..Default::default()
                }
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
                let language = metadata.get("language").cloned();
                jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Audio),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    channels: nonzero(*nb_channels).map(|v| v as i64),
                    sample_rate: nonzero(*sample_rate).map(|v| v as i64),
                    bit_rate: nonzero(*bit_rate),
                    is_default: Some(check_disposition(metadata, "default")),
                    is_forced: Some(check_disposition(metadata, "forced")),
                    display_title: display_title(
                        language.as_deref(),
                        Some(codec_name),
                        "Audio",
                        nonzero(*nb_channels),
                    ),
                    language,
                    title: metadata.get("title").cloned(),
                    ..Default::default()
                }
            }
            StreamInfo::Subtitle {
                index,
                codec_name,
                metadata,
                ..
            } => {
                let language = metadata.get("language").cloned();
                jellyfin::MediaStream {
                    type_: Some(jellyfin::MediaStreamType::Subtitle),
                    index: Some(*index as i64),
                    codec: Some(codec_name.clone()),
                    is_default: Some(check_disposition(metadata, "default")),
                    is_forced: Some(check_disposition(metadata, "forced")),
                    display_title: display_title(
                        language.as_deref(),
                        Some(codec_name),
                        "Subtitle",
                        None,
                    ),
                    language,
                    title: metadata.get("title").cloned(),
                    ..Default::default()
                }
            }
            _ => continue,
        };

        streams.push(stream);
    }

    let run_time_ticks = duration_us.and_then(|us| us.checked_mul(10));
    let container = format.map(|f| map_container(&f));

    Ok(jellyfin::MediaSourceInfo {
        media_streams: streams,
        container,
        run_time_ticks,
        bitrate,
        //  supports_direct_play: Some(true),
        //  supports_direct_stream: Some(true),
        //supports_transcoding: Some(true),
        ..Default::default()
    })
}
