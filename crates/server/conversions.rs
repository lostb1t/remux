use crate::db;
use crate::jellyfin;
use crate::sdks::aio;
use crate::utils;
use crate::utils::get_uuid;
use anyhow::Result;
use std::convert::{TryFrom, TryInto};

impl From<jellyfin::DisplayPreferencesDto> for db::JellyfinDisplayPrefsData {
    fn from(dto: jellyfin::DisplayPreferencesDto) -> Self {
        Self {
            view_type: dto.view_type,
            sort_by: dto.sort_by,
            index_by: dto.index_by,
            remember_indexing: dto.remember_indexing,
            primary_image_height: dto.primary_image_height,
            primary_image_width: dto.primary_image_width,
            custom_prefs: dto.custom_prefs,
            scroll_direction: dto
                .scroll_direction
                .map(|d| d.to_string())
                .unwrap_or_else(|| "Horizontal".to_string()),
            show_backdrop: dto.show_backdrop,
            remember_sorting: dto.remember_sorting,
            sort_order: dto
                .sort_order
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Ascending".to_string()),
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

pub fn subtitle_to_media_stream(sub: aio::Subtitle) -> jellyfin::MediaStream {
    let lc = sub.url.to_ascii_lowercase();
    let codec = if lc.ends_with(".vtt") {
        "webvtt"
    } else if lc.ends_with(".srt") {
        "subrip"
    } else {
        "webvtt"
    };
    jellyfin::MediaStream {
        index: Some(0),
        type_: Some(jellyfin::MediaStreamType::Subtitle),
        codec: Some(codec.to_string()),
        language: sub.lang.clone(),
        display_title: Some({
            let lang = sub.lang.clone().unwrap_or_else(|| "und".into());
            format!("{} - {} - External", lang, codec.to_uppercase())
        }),
        is_default: Some(false),
        is_forced: Some(false),
        is_external: Some(true),
        is_text_subtitle_stream: Some(true),
        delivery_url: Some(sub.url.clone()),
        is_external_url: Some(true),
        ..Default::default()
    }
}

pub fn stream_into_media_source_info(
    id: String,
    jellyfin_media_type: jellyfin::MediaType,
    stream: aio::Stream,
) -> jellyfin::MediaSourceInfo {
    let id = get_uuid();
    jellyfin::MediaSourceInfo {
        id: id.clone(),
        e_tag: Some(id.clone()),
        path: stream.url,
        protocol: Some("File".to_string()),
        supports_transcoding: Some(false),
        supports_direct_stream: Some(true),
        supports_direct_play: Some(true),
        is_remote: Some(false),
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
