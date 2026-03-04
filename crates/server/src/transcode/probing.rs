use crate::jellyfin;
use anyhow::{Result, anyhow};
use gstreamer as gst;
use gstreamer_pbutils as gst_pbutils;
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

/// Map a GStreamer caps structure name to a Jellyfin codec name.
fn map_video_codec(caps: &gst::Caps) -> Option<String> {
    let s = caps.structure(0)?;
    match s.name().as_str() {
        "video/x-h264" => Some("h264".into()),
        "video/x-h265" => Some("hevc".into()),
        "video/x-vp8" => Some("vp8".into()),
        "video/x-vp9" => Some("vp9".into()),
        "video/x-av1" => Some("av1".into()),
        "video/mpeg" => {
            let ver: i32 = s.get("mpegversion").unwrap_or(2);
            if ver == 4 { Some("mpeg4".into()) } else { Some("mpeg2video".into()) }
        }
        other => Some(other.trim_start_matches("video/x-").to_string()),
    }
}

fn map_audio_codec(caps: &gst::Caps) -> Option<String> {
    let s = caps.structure(0)?;
    match s.name().as_str() {
        "audio/mpeg" => {
            let ver: i32 = s.get("mpegversion").unwrap_or(1);
            match ver {
                4 | 2 => Some("aac".into()),
                _ => Some("mp3".into()),
            }
        }
        "audio/x-ac3" => Some("ac3".into()),
        "audio/x-eac3" => Some("eac3".into()),
        "audio/x-dts" => Some("dts".into()),
        "audio/x-flac" => Some("flac".into()),
        "audio/x-opus" => Some("opus".into()),
        "audio/x-vorbis" => Some("vorbis".into()),
        "audio/x-raw" => Some("pcm".into()),
        "audio/x-alac" => Some("alac".into()),
        other => Some(other.trim_start_matches("audio/x-").to_string()),
    }
}

fn map_subtitle_codec(caps: &gst::Caps) -> Option<String> {
    let s = caps.structure(0)?;
    match s.name().as_str() {
        "application/x-ssa" | "text/x-ssa" => Some("ass".into()),
        "application/x-ass" => Some("ass".into()),
        "application/x-subtitle-srt" | "application/x-subrip" => Some("subrip".into()),
        "subpicture/x-dvd" => Some("dvdsub".into()),
        "subpicture/x-pgs" => Some("pgssub".into()),
        "text/x-raw" => Some("srt".into()),
        other => Some(other.rsplit('/').next().unwrap_or(other).trim_start_matches("x-").to_string()),
    }
}

/// Map a GStreamer container caps name to a Jellyfin container name.
fn map_container(caps: &gst::Caps) -> String {
    let Some(s) = caps.structure(0) else {
        return "unknown".into();
    };
    match s.name().as_str() {
        "video/x-matroska" => "mkv".into(),
        "video/webm" => "webm".into(),
        "video/quicktime" => {
            let variant: Option<String> = s.get("variant").ok();
            match variant.as_deref() {
                Some("iso") | Some("iso-fragmented") => "mp4".into(),
                _ => "mov".into(),
            }
        }
        "video/mpegts" => "ts".into(),
        "video/x-msvideo" => "avi".into(),
        "application/ogg" => "ogg".into(),
        other => other.rsplit('/').next().unwrap_or("unknown").trim_start_matches("x-").to_string(),
    }
}

/// Probe a media URL and return a Jellyfin MediaSourceInfo directly.
pub fn probe_media(url: &str) -> Result<jellyfin::MediaSourceInfo> {
    use gst_pbutils::prelude::*;

    tracing::debug!(url, "probing media");

    let timeout = gst::ClockTime::from_seconds(30);
    let discoverer = gst_pbutils::Discoverer::new(timeout)
        .map_err(|e| anyhow!("Failed to create GStreamer Discoverer: {}", e))?;

    let info = discoverer
        .discover_uri(url)
        .map_err(|e| anyhow!("Failed to discover media {}: {}", url, e))?;

    let duration = info.duration();
    let run_time_ticks = duration.map(|d| d.nseconds() as i64 / 100);

    // Container info from the top-level stream
    let container = info
        .stream_info()
        .and_then(|si| si.caps())
        .map(|c| map_container(&c));

    tracing::debug!(?duration, ?container, "probe container info");

    let mut streams: Vec<jellyfin::MediaStream> = Vec::new();
    let mut overall_bitrate: Option<i64> = None;

    // Video streams
    for (idx, video) in info.video_streams().iter().enumerate() {
        let caps = video.caps();
        let codec = caps.as_ref().and_then(|c| map_video_codec(c));
        let bitrate = nonzero(video.bitrate() as i64)
            .or_else(|| nonzero(video.max_bitrate() as i64));
        if overall_bitrate.is_none() {
            overall_bitrate = bitrate;
        }

        let framerate = video.framerate();
        let fps = if framerate.denom() > 0 {
            Some(framerate.numer() as f32 / framerate.denom() as f32)
        } else {
            None
        };

        let tags = video.tags();
        let language = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::LanguageCode>().map(|v| v.get().to_string())
        });
        let title = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::Title>().map(|v| v.get().to_string())
        });

        streams.push(jellyfin::MediaStream {
            type_: Some(jellyfin::MediaStreamType::Video),
            index: Some(idx as i64),
            codec: codec.clone(),
            width: nonzero(video.width() as i64),
            height: nonzero(video.height() as i64),
            bit_rate: bitrate,
            average_frame_rate: fps.and_then(nonzero),
            real_frame_rate: fps.and_then(nonzero),
            is_default: Some(idx == 0),
            is_forced: Some(false),
            display_title: display_title(
                language.as_deref(),
                codec.as_deref(),
                "Video",
                None,
            ),
            language,
            title,
            ..Default::default()
        });
    }

    // Audio streams
    for (idx, audio) in info.audio_streams().iter().enumerate() {
        let caps = audio.caps();
        let codec = caps.as_ref().and_then(|c| map_audio_codec(c));
        let bitrate = nonzero(audio.bitrate() as i64)
            .or_else(|| nonzero(audio.max_bitrate() as i64));
        let channels = nonzero(audio.channels() as i32);

        let tags = audio.tags();
        let language = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::LanguageCode>().map(|v| v.get().to_string())
        });
        let title = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::Title>().map(|v| v.get().to_string())
        });

        streams.push(jellyfin::MediaStream {
            type_: Some(jellyfin::MediaStreamType::Audio),
            index: Some(idx as i64),
            codec: codec.clone(),
            channels: channels.map(|v| v as i64),
            sample_rate: nonzero(audio.sample_rate() as i64),
            bit_rate: bitrate,
            is_default: Some(idx == 0),
            is_forced: Some(false),
            display_title: display_title(
                language.as_deref(),
                codec.as_deref(),
                "Audio",
                channels,
            ),
            language,
            title,
            ..Default::default()
        });
    }

    // Subtitle streams
    for (idx, sub) in info.subtitle_streams().iter().enumerate() {
        let caps = sub.caps();
        let codec = caps.as_ref().and_then(|c| map_subtitle_codec(c));

        let tags = sub.tags();
        let language = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::LanguageCode>().map(|v| v.get().to_string())
        });
        let title = tags.as_ref().and_then(|t| {
            t.get::<gst::tags::Title>().map(|v| v.get().to_string())
        });

        streams.push(jellyfin::MediaStream {
            type_: Some(jellyfin::MediaStreamType::Subtitle),
            index: Some(idx as i64),
            codec: codec.clone(),
            is_default: Some(idx == 0),
            is_forced: Some(false),
            display_title: display_title(
                language.as_deref(),
                codec.as_deref(),
                "Subtitle",
                None,
            ),
            language,
            title,
            ..Default::default()
        });
    }

    // Fall back to container-level bitrate if no stream had one
    if overall_bitrate.is_none() {
        if let Some(tags) = info.tags() {
            overall_bitrate = tags
                .get::<gst::tags::Bitrate>()
                .map(|v| v.get() as i64)
                .and_then(nonzero)
                .or_else(|| {
                    tags.get::<gst::tags::NominalBitrate>()
                        .map(|v| v.get() as i64)
                        .and_then(nonzero)
                });
        }
    }

    Ok(jellyfin::MediaSourceInfo {
        media_streams: streams,
        container,
        run_time_ticks,
        bitrate: overall_bitrate,
        ..Default::default()
    })
}
