use crate::{
    addons::SubtitleInfo,
    api, common,
    common::{ToRunTimeTicks, get_uuid},
    db,
    sdks::stremio,
    stream::StreamDescriptor,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

// Heuristic metadata fallback for remote source URLs when ffprobe metadata is
// unavailable. This keeps clients functional (stream selection/transcode
// decisions) instead of exposing empty stream lists.
fn infer_container_from_url(url: &str) -> Option<String> {
    let path = url::Url::parse(url)
        .ok()
        .map(|u| {
            u.path()
                .to_string()
        })
        .unwrap_or_else(|| url.to_string());
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or(path.as_str());
    let ext = filename
        .rsplit('.')
        .next()?
        .to_ascii_lowercase();
    match ext.as_str() {
        "matroska" | "mkv" => Some("mkv".to_string()),
        "mp4" | "m4v" | "mov" => Some("mp4".to_string()),
        "webm" => Some("webm".to_string()),
        "avi" => Some("avi".to_string()),
        "m2ts" | "ts" => Some("ts".to_string()),
        "m3u8" => Some("ts".to_string()),
        _ => None,
    }
}

fn infer_video_codec(text: &str) -> Option<String> {
    if text.contains("hevc") || text.contains("h265") || text.contains("x265") {
        Some("hevc".to_string())
    } else if text.contains("av1") {
        Some("av1".to_string())
    } else if text.contains("vp9") {
        Some("vp9".to_string())
    } else if text.contains("h264") || text.contains("x264") || text.contains("avc") {
        Some("h264".to_string())
    } else {
        None
    }
}

fn infer_audio_codec(text: &str) -> Option<String> {
    if text.contains("truehd") {
        Some("truehd".to_string())
    } else if text.contains("dts") || text.contains("dca") {
        Some("dts".to_string())
    } else if text.contains("eac3") || text.contains("ddp") {
        Some("eac3".to_string())
    } else if text.contains("ac3") {
        Some("ac3".to_string())
    } else if text.contains("aac") {
        Some("aac".to_string())
    } else {
        None
    }
}

fn infer_audio_channels(text: &str) -> Option<i64> {
    if text.contains("7.1") {
        Some(8)
    } else if text.contains("5.1") {
        Some(6)
    } else if text.contains("2.0") || text.contains("stereo") {
        Some(2)
    } else {
        None
    }
}

fn fallback_media_streams(source: &db::Media) -> Vec<api::MediaStream> {
    // Only synthesize streams for remote source entries.
    if source.kind != db::MediaKind::Stream {
        return Vec::new();
    }

    if !source.is_remote_url() {
        return Vec::new();
    }

    let text = source
        .title
        .to_ascii_lowercase();
    let video_codec = infer_video_codec(&text);
    let audio_codec = infer_audio_codec(&text);
    let channels = infer_audio_channels(&text);

    let video_title = video_codec
        .as_ref()
        .map(|c| format!("{} - Fallback", c.to_uppercase()))
        .unwrap_or_else(|| "Video - Fallback".to_string());
    let audio_title = match (&audio_codec, channels) {
        (Some(c), Some(8)) => format!("{} - 7.1 - Fallback", c.to_uppercase()),
        (Some(c), Some(6)) => format!("{} - 5.1 - Fallback", c.to_uppercase()),
        (Some(c), Some(2)) => format!("{} - Stereo - Fallback", c.to_uppercase()),
        (Some(c), _) => format!("{} - Fallback", c.to_uppercase()),
        (None, Some(8)) => "Audio - 7.1 - Fallback".to_string(),
        (None, Some(6)) => "Audio - 5.1 - Fallback".to_string(),
        (None, Some(2)) => "Audio - Stereo - Fallback".to_string(),
        (None, _) => "Audio - Fallback".to_string(),
    };

    vec![
        api::MediaStream {
            type_: Some(api::MediaStreamType::Video),
            codec: video_codec,
            is_default: Some(true),
            display_title: Some(video_title),
            ..Default::default()
        },
        api::MediaStream {
            type_: Some(api::MediaStreamType::Audio),
            codec: audio_codec,
            channels,
            is_default: Some(true),
            display_title: Some(audio_title),
            ..Default::default()
        },
    ]
}

impl From<db::Media> for api::MediaSourceInfo {
    fn from(source: db::Media) -> Self {
        let descriptor = source
            .stream_info
            .as_ref()
            .map(|si| &si.descriptor);
        let is_stub = descriptor
            .and_then(|d| d.as_http_url())
            .is_none();
        let container = descriptor
            .and_then(|d| d.as_http_url())
            .and_then(infer_container_from_url);

        let remux = Some(api::MediaSourceRemuxInfo {
            provider_info: source
                .stream_info
                .as_ref()
                .and_then(|si| serde_json::to_value(si).ok()),
        });

        let path = Some({
            let stem = source
                .stream_info
                .as_ref()
                .and_then(|si| {
                    si.filename
                        .as_deref()
                })
                .and_then(|f| {
                    std::path::Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                });
            match stem {
                Some(s) => format!("/remux/{}/{}", source.id, s),
                None => format!("/remux/{}", source.id),
            }
        });
        let is_remote = false;
        let protocol = api::MediaProtocol::File;

        let client_id = source
            .group_id
            .unwrap_or(source.id);
        let probe_ticks = source
            .probe_data
            .as_ref()
            .and_then(|p| p.run_time_ticks);
        let meta_ticks = source
            .runtime
            .and_then(|r| r.to_ticks(common::TickUnit::Seconds));
        let run_time_ticks = probe_ticks.or(meta_ticks);
        let (
            mut media_streams,
            default_audio_stream_index,
            default_subtitle_stream_index,
        ) = source
            .probe_data
            .map(|p| {
                (
                    p.media_streams,
                    p.default_audio_stream_index,
                    p.default_subtitle_stream_index,
                )
            })
            .unwrap_or_default();

        // Clients that use /Items/{id}/File for direct playback inspect
        // MediaStreams before deciding to play. Synthesize a stub so they
        // don't reject unprobed tracks outright.
        if source.kind == db::MediaKind::Track && media_streams.is_empty() {
            media_streams = vec![api::MediaStream {
                type_: Some(api::MediaStreamType::Audio),
                codec: Some("aac".to_string()),
                channels: Some(2),
                is_default: Some(true),
                display_title: Some("Audio".to_string()),
                index: 0,
                ..Default::default()
            }];
        }
        api::MediaSourceInfo {
            id: client_id,
            e_tag: client_id,
            path,
            protocol,
            is_remote,
            name: Some(
                source
                    .title
                    .clone(),
            ),
            container,
            remux,
            has_segments: !is_stub,
            formats: Some(vec![]),
            required_http_headers: Some(HashMap::new()),
            run_time_ticks,
            media_streams,
            default_audio_stream_index,
            default_subtitle_stream_index,
            ..Default::default()
        }
    }
}
impl From<api::DisplayPreferencesDto> for db::JellyfinDisplayPrefsData {
    fn from(dto: api::DisplayPreferencesDto) -> Self {
        Self {
            view_type: dto.view_type,
            sort_by: dto.sort_by,
            index_by: dto.index_by,
            remember_indexing: dto.remember_indexing,
            primary_image_height: dto.primary_image_height,
            primary_image_width: dto.primary_image_width,
            custom_prefs: dto.custom_prefs,
            scroll_direction: dto.scroll_direction,
            show_backdrop: dto.show_backdrop,
            remember_sorting: dto.remember_sorting,
            sort_order: dto.sort_order,
            show_sidebar: dto.show_sidebar,
            home_sections: None,
        }
    }
}

impl TryFrom<stremio::Episode> for db::Media {
    type Error = anyhow::Error;
    fn try_from(meta: stremio::Episode) -> Result<db::Media> {
        let mut media = db::Media {
            title: meta
                .get_name()
                .unwrap_or_default(),
            kind: db::MediaKind::Episode,
            released_at: meta
                .released
                .map(|x| x.naive_utc()),
            runtime: meta
                .runtime
                .map(|d| d.num_seconds()),
            description: meta
                .overview
                .or(meta.description),
            rating_audience: meta.rating,
            ..Default::default()
        };
        if let Some(url) = meta.thumbnail {
            media.set_image(db::ImageKind::Primary, url);
        }
        Ok(media)
    }
}

pub fn subtitle_to_media_stream(sub: &SubtitleInfo) -> api::MediaStream {
    let path_hint = match &sub.url {
        Some(StreamDescriptor::Http { url, .. }) => url.as_str(),
        Some(StreamDescriptor::Local(p)) => p
            .to_str()
            .unwrap_or(""),
        Some(StreamDescriptor::Opendal { path, .. }) => path.as_str(),
        _ => "",
    };
    let lc = path_hint.to_ascii_lowercase();
    let codec = if lc.ends_with(".vtt") {
        "webvtt"
    } else if lc.ends_with(".srt") {
        "subrip"
    } else if lc.ends_with(".ass") || lc.ends_with(".ssa") {
        "ass"
    } else {
        "webvtt"
    };
    api::MediaStream {
        index: 0,
        type_: Some(api::MediaStreamType::Subtitle),
        codec: Some(codec.to_string()),
        language: sub
            .lang
            .clone(),
        display_title: Some({
            let lang = sub
                .lang
                .clone()
                .unwrap_or_else(|| "und".into());
            format!("{} - {} - External", lang, codec.to_uppercase())
        }),
        is_default: Some(false),
        is_forced: sub.is_forced,
        is_hearing_impaired: sub.is_hi,
        is_external: true,
        is_text_subtitle_stream: true,
        supports_external_stream: true,
        delivery_method: Some(api::SubtitleDeliveryMethod::External),
        is_external_url: Some(false),
        audio_spatial_format: Some("None".to_string()),
        video_range: Some(api::VideoRange::Unknown),
        video_range_type: Some(api::VideoRangeType::Unknown),
        localized_undefined: Some("Undefined".to_string()),
        localized_default: Some("Default".to_string()),
        localized_forced: Some("Forced".to_string()),
        localized_external: Some("External".to_string()),
        localized_hearing_impaired: Some("Hearing Impaired".to_string()),
        ..Default::default()
    }
}

pub fn stream_into_media_source_info(
    id: String,
    jellyfin_media_type: api::MediaType,
    stream: stremio::Stream,
) -> api::MediaSourceInfo {
    let id = get_uuid();
    api::MediaSourceInfo {
        id: id.clone(),
        e_tag: id.clone(),
        path: stream.url,
        protocol: api::MediaProtocol::File,
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_remote: false,
        name: stream
            .name
            .clone(),
        ..Default::default()
    }
}

fn to_option_bool(flag: i64) -> Option<bool> {
    match flag {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

// --- Subtitle text conversion ---
//
// Jellyfin-web fetches subtitles as either JSON TrackEvents (Stream.js)
// or WebVTT (Stream.vtt).  We extract to SRT via ffmpeg and convert.

/// Convert SRT to WebVTT. Already-valid VTT is passed through unchanged.
pub fn srt_to_vtt(input: &str) -> String {
    let input = input.trim_start_matches('\u{FEFF}');
    if input
        .trim_start()
        .starts_with("WEBVTT")
    {
        // If there's a second WEBVTT header mid-file (e.g. OpenSubtitles metadata
        // block), drop everything before it — the real cues start there.
        let second = input
            .find("WEBVTT")
            .and_then(|first| {
                input[first + 6..]
                    .find("WEBVTT")
                    .map(|off| first + 6 + off)
            });
        if let Some(pos) = second {
            return input[pos..]
                .trim_start_matches('\u{FEFF}')
                .to_string();
        }
        return input.to_string();
    }
    let mut out = String::from("WEBVTT\n\n");
    for block in input
        .trim()
        .split("\n\n")
    {
        let lines: Vec<&str> = block
            .lines()
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let rest = if lines[0]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            &lines[1..]
        } else {
            &lines[..]
        };
        if rest.is_empty() {
            continue;
        }
        let timecode = rest[0].replace(',', ".");
        out.push_str(&timecode);
        out.push('\n');
        for line in &rest[1..] {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Convert SRT to Jellyfin JSON TrackEvents format (1 tick = 100 ns).
pub fn srt_to_jellyfin_json(input: &str) -> String {
    let mut events: Vec<serde_json::Value> = Vec::new();
    for block in input
        .trim()
        .split("\n\n")
    {
        let lines: Vec<&str> = block
            .lines()
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let content = if lines[0]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            &lines[1..]
        } else {
            &lines[..]
        };
        if content.is_empty() {
            continue;
        }
        let parts: Vec<&str> = content[0]
            .split("-->")
            .collect();
        if parts.len() < 2 {
            continue;
        }
        let start = srt_timestamp_to_ticks(parts[0].trim());
        let end = srt_timestamp_to_ticks(parts[1].trim());
        let text = content[1..].join("\n");
        if let (Some(s), Some(e)) = (start, end) {
            events.push(serde_json::json!({
                "Id": events.len().to_string(),
                "Text": text,
                "StartPositionTicks": s,
                "EndPositionTicks": e,
            }));
        }
    }
    serde_json::json!({ "TrackEvents": events }).to_string()
}

fn srt_timestamp_to_ticks(ts: &str) -> Option<i64> {
    let cleaned = ts.replace(',', ".");
    let parts: Vec<&str> = cleaned
        .split(':')
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let h: i64 = parts[0]
        .parse()
        .ok()?;
    let m: i64 = parts[1]
        .parse()
        .ok()?;
    let sp: Vec<&str> = parts[2]
        .split('.')
        .collect();
    let s: i64 = sp[0]
        .parse()
        .ok()?;
    let ms: i64 = if sp.len() > 1 {
        let padded = format!("{:0<3}", sp[1]);
        padded[..3]
            .parse()
            .ok()?
    } else {
        0
    };
    Some(((h * 3600 + m * 60 + s) * 1000 + ms) * 10_000)
}
