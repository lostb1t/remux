use crate::db;
use crate::api;
use crate::sdks::aio;
use crate::utils;
use crate::utils::get_uuid;
use anyhow::Result;
use std::convert::{TryFrom, TryInto};

// Heuristic metadata fallback for remote source URLs when ffprobe metadata is
// unavailable. This keeps clients functional (stream selection/transcode
// decisions) instead of exposing empty stream lists.
fn infer_container_from_url(url: &str) -> Option<String> {
    let path = url::Url::parse(url)
        .ok()
        .map(|u| u.path().to_string())
        .unwrap_or_else(|| url.to_string());
    let filename = path.rsplit('/').next().unwrap_or(path.as_str());
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
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
    if source.kind != db::MediaKind::Source {
        return Vec::new();
    }

    if !source.is_remote_url() {
        return Vec::new();
    }

    let text = source.title.to_ascii_lowercase();
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
        let is_remote = source.is_remote_url();
        let protocol = source.media_source_protocol().to_string();
        let container = source.url.as_deref().and_then(infer_container_from_url);
        let media_streams = fallback_media_streams(&source);

        let clean_path = source
            .url
            .as_ref()
            .and_then(|u| {
                url::Url::parse(u).ok().and_then(|parsed| {
                    parsed.path_segments()?.last().map(|s| s.to_string())
                })
            })
            .unwrap_or_else(|| source.title.clone());

        api::MediaSourceInfo {
            id: source.id.clone(),
            e_tag: source.id.clone(),
            path: source.url.clone(),
            protocol,
            supports_transcoding: false,
            supports_direct_stream: true,
            supports_direct_play: true,
            is_remote,
            name: Some(source.title.clone()),
            container,
            media_streams,
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

impl TryFrom<aio::Episode> for db::Media {
    type Error = anyhow::Error;
    fn try_from(meta: aio::Episode) -> Result<db::Media> {
        Ok(db::Media {
            title: meta.title.unwrap_or_default(),
            kind: db::MediaKind::Episode,
            released_at: meta.released.map(|x| x.naive_utc()),
            runtime: meta.runtime.map(|d| d.num_seconds()),
            description: meta.overview,
            poster: meta.thumbnail,
            ..Default::default()
        })
    }
}

pub fn subtitle_to_media_stream(sub: aio::Subtitle) -> api::MediaStream {
    let lc = sub.url.to_ascii_lowercase();
    let codec = if lc.ends_with(".vtt") {
        "webvtt"
    } else if lc.ends_with(".srt") {
        "subrip"
    } else {
        "webvtt"
    };
    api::MediaStream {
        index: 0,
        type_: Some(api::MediaStreamType::Subtitle),
        codec: Some(codec.to_string()),
        language: sub.lang.clone(),
        display_title: Some({
            let lang = sub.lang.clone().unwrap_or_else(|| "und".into());
            format!("{} - {} - External", lang, codec.to_uppercase())
        }),
        is_default: Some(false),
        is_forced: false,
        is_external: true,
        is_text_subtitle_stream: true,
        delivery_url: Some(sub.url.clone()),
        is_external_url: Some(true),
        ..Default::default()
    }
}

pub fn stream_into_media_source_info(
    id: String,
    jellyfin_media_type: api::MediaType,
    stream: aio::Stream,
) -> api::MediaSourceInfo {
    let id = get_uuid();
    api::MediaSourceInfo {
        id: id.clone(),
        e_tag: id.clone(),
        path: stream.url,
        protocol: "File".to_string(),
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_remote: false,
        name: stream.name.clone(),
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
