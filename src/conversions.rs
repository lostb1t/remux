use crate::db;
use crate::jellyfin;
use crate::sdks::{aio, tmdb};
use crate::utils;
//use crate::utils::MediaId;
use crate::utils::get_uuid;
use crate::utils::server_id;
use crate::utils::{IntoVec, ToRunTimeTicks};
use anyhow::{Error, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use chrono::{DateTime, FixedOffset, Utc};
use isolang::Language;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;

impl From<db::JellyfinDisplayPrefs> for jellyfin::DisplayPreferencesDto {
    fn from(prefs: db::JellyfinDisplayPrefs) -> Self {
        let data = prefs.data;

        Self {
            id: Some(prefs.id),
            view_type: data.view_type.clone(),
            sort_by: data.sort_by.clone(),
            index_by: data.index_by.clone(),
            remember_indexing: data.remember_indexing,
            primary_image_height: data.primary_image_height,
            primary_image_width: data.primary_image_width,
            custom_prefs: data.custom_prefs.clone(),
            scroll_direction: data.scroll_direction.clone(),
            show_backdrop: data.show_backdrop,
            remember_sorting: data.remember_sorting,
            sort_order: data.sort_order.clone(),
            show_sidebar: data.show_sidebar,
            client: prefs.client,
        }
    }
}

impl From<db::Media> for jellyfin::BaseItemDto {
    fn from(media: db::Media) -> Self {
        // dbg!(&meta);
        // let media_type: jellyfin::MediaType = meta.media_type.clone().into();

        let mut item = jellyfin::BaseItemDto {
            id: media.id.clone(),
            etag: Some(media.id),
            // id: get_stable_uuid(meta.id.clone()),
            server_id: utils::server_id(),
            name: Some(media.title.clone()),
            original_title: Some(media.title.clone()),
            overview: media.description.clone(),
            type_: media.kind.clone().into(),
            parent_id: media.parent_id.clone(),
            remote_trailers: media.trailers.clone().map(|j| j.0),
            // might be better to save it aa a column on media.
            //series_id: if media.kind == db::MediaKind::Season
            // {
            //     media.parent_id
            //} else {
            //    None
            //
            series_id: media.parent_id,
            season_id: media.parent_id,
            //  season_name: Some(media.title.clone()),
            //  series_name: Some(media.title.clone()),
            //  user_data: Some(jellyfin::UserItemDataDto::default()),

            // collection_type: {
            //     if media.kind == db::MediaKind::Catalog {
            //         match media.kind.as_str() {
            //             "series" => Some(jellyfin::CollectionType::Tvshows),
            //             "movie" => Some(jellyfin::CollectionType::Movies),
            //             _ => None,
            //         }
            //     } else {
            //         None
            //     }
            // },
            is_place_holder: media.sources.as_ref().map(|sources| sources.is_empty()),
            premiere_date: media.released_at.clone().map(|d| d.and_utc()),
            // community_rating: meta.imdb_rating.clone().and_then(|r| r.parse().ok()),
            image_tags: Some(jellyfin::ImageTags {
                primary: media.poster.clone(),
                logo: media.logo.clone(),
                backdrop: media.backdrop.clone(),
                ..Default::default()
            }),
            index_number: media.idx,
            //           series_id: Some(item.id.clone()),
            //            season_id: Some(season.id.clone()),
            is_folder: if media.kind == db::MediaKind::Series
                || media.kind == db::MediaKind::Catalog
                || media.kind == db::MediaKind::Season
            {
                true
            } else {
                false
            },
            backdrop_image_tags: media.backdrop.clone().map(|url| vec![url]),
            // image_blur_hashes: Some(jellyfin::ImageBlurHashes {
            //     backdrop: {
            //         if let Some(img) = meta.background.clone() {
            //             Some(HashMap::from([(img.clone(), img)]))
            //             // Some(HashMap::from([("3626323".to_string, img)]))
            //         } else {
            //             None
            //         }
            //     },
            //     primary: {
            //         if let Some(img) = meta.poster.clone() {
            //             Some(HashMap::from([(img.clone(), img)]))
            //         } else {
            //             None
            //         }
            //     },
            //     logo: {
            //         if let Some(img) = meta.logo.clone() {
            //             Some(HashMap::from([(img.clone(), img)]))
            //         } else {
            //             None
            //         }
            //     },
            //     ..Default::default()
            // }),
            provider_ids: Some(jellyfin::ProviderIds {
                imdb: media.imdb_id.clone(),
                ..Default::default()
            }),
            //genres: meta.genres.clone(),
            run_time_ticks: media
                .runtime
                .map(|r| r.to_ticks(utils::TickUnit::Seconds).unwrap()),

            // only load sources from "prefetch"
            ..Default::default()
        };

        if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
            item.media_sources = match media.sources.clone() {
                Some(sources) if sources.is_empty() => Some(vec![media.clone().into()]),
                Some(sources) => Some(
                    sources
                        .into_iter()
                        .map(jellyfin::MediaSourceInfo::from)
                        .collect(),
                ),
                None => None,
            };
        }
        // this shouldnt be done here but eh
        item
    }
}

//Resources

// impl From<aio::Stream> for jellyfin::MediaSourceInfo {
//     fn from(item: aio::Stream) -> Self {
//         let id = Some(URL_SAFE.encode(item.url.unwrap()));

//         //let streams =

//         Self {
//             // base64 encode url
//             //id: Some("yoo".to_string()),
//             id: id.clone(),
//             e_tag: id,
//             name: Some(item.name.unwrap()),

//             supports_direct_play: Some(true),
//             supports_direct_stream: Some(true),
//             ..Default::default()
//         }
//     }
// }

impl From<aio::Subtitle> for jellyfin::MediaStream {
    fn from(sub: aio::Subtitle) -> Self {
        // Guess codec from URL extension; default to webvtt for browser compat
        let lc = sub.url.to_ascii_lowercase();
        let codec = if lc.ends_with(".vtt") {
            "webvtt"
        } else if lc.ends_with(".srt") {
            "subrip"
        } else {
            "webvtt"
        };

        // Build a single external text subtitle stream
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
            // delivery_method: Some(jellyfin::SubtitleDeliveryMethod::External),
            delivery_url: Some(sub.url.clone()),
            is_external_url: Some(true),
            ..Default::default()
        }

        // Represent this subtitle as a MediaSourceInfo that supports external streams
        // let id = Some(URL_SAFE.encode(sub.id));
        // jellyfin::MediaSourceInfo {
        //     id: id.clone(),
        //     e_tag: id,
        //     name: Some("External Subtitle".to_string()),
        //     supports_direct_play: Some(true),
        //     supports_direct_stream: Some(true),
        //     // supports_external_stream: Some(true),
        //     media_streams: Some(vec![stream]),
        //     ..Default::default()
        // }
    }
}

impl From<db::User> for jellyfin::UserDto {
    fn from(user: db::User) -> Self {
        jellyfin::UserDto {
            server_id: server_id(),
            name: user.username,
            id: user.id,
            ..Default::default()
        }
    }
}

impl From<aio::Catalog> for jellyfin::BaseItemDto {
    fn from(item: aio::Catalog) -> Self {
        jellyfin::BaseItemDto {
            name: Some(item.name.clone()),

            id: get_uuid(),
            type_: jellyfin::MediaType::BoxSet,
            ..Default::default()
        }
    }
}

impl From<aio::MediaType> for jellyfin::MediaType {
    fn from(kind: aio::MediaType) -> Self {
        match kind {
            aio::MediaType::Movie => jellyfin::MediaType::Movie,
            aio::MediaType::Series => jellyfin::MediaType::Series,
            _ => jellyfin::MediaType::Unknown,
        }
    }
}

impl From<jellyfin::MediaType> for aio::MediaType {
    fn from(kind: jellyfin::MediaType) -> Self {
        match kind {
            jellyfin::MediaType::Movie => aio::MediaType::Movie,
            jellyfin::MediaType::Series => aio::MediaType::Series,
            _ => aio::MediaType::Unknown,
        }
    }
}

pub fn stream_into_media_source_info(
    id: String,
    jellyfin_media_type: jellyfin::MediaType,
    stream: aio::Stream,
) -> jellyfin::MediaSourceInfo {
    //let id = get_uuid();
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

use ffprobe;
//use base64::engine::general_purpose::URL_SAFE;
//use base64::Engine;

impl From<ffprobe::FfProbe> for jellyfin::MediaSourceInfo {
    fn from(probe: ffprobe::FfProbe) -> Self {
        let streams: Vec<jellyfin::MediaStream> = probe
            .streams
            .into_iter()
            .map(|s| {
                //dbg!(&s);
                let language = s.tags.as_ref().and_then(|tags| tags.language.clone());
                jellyfin::MediaStream {
                    aspect_ratio: s.display_aspect_ratio,
                    //average_frame_rate: s.avg_frame_rate,
                    bit_rate: s.bit_rate.map(|x| x.parse::<i64>().unwrap()),
                    codec: s.codec_name.clone(),
                    codec_tag: Some(s.codec_tag),
                    //codec_time_base: s.codec_time_base,
                    height: s.height.map(|x| x as i64),
                    width: s.width.map(|x| x as i64),
                    channels: s.channels.map(|x| x as i64),
                    channel_layout: s.channel_layout.clone(),
                    //sample_rate: s.sample_rate,
                    //time_base: s.time_base,
                    display_title: {
                        let mut parts = vec![];
                        parts.push(
                            language
                                .as_deref()
                                .and_then(|code| Language::from_str(code).ok())
                                .map(|lang| lang.to_name().to_string()),
                        );

                        parts.push(s.codec_name.as_ref().map(|s| s.to_string()));

                        if s.codec_type.as_deref() == Some("audio") {
                            parts.push(s.channel_layout.map(|c| c.to_string()));
                        }

                        let joined =
                            parts.into_iter().flatten().collect::<Vec<_>>().join(" - ");
                        if joined.is_empty() {
                            None
                        } else {
                            Some(joined)
                        }
                    },
                    index: Some(s.index as i64),
                    language,
                    //language: s.tags.as_ref().and_then(|tags| tags.get("language").cloned()),
                    is_default: to_option_bool(s.disposition.default),
                    is_forced: to_option_bool(s.disposition.forced),
                    is_hearing_impaired: to_option_bool(s.disposition.hearing_impaired),
                    // is_avc: s.is_avc,
                    //profile: s.profile,
                    // pixel_format: s.pix_fmt,
                    //level: s.level.map(|v| v as f64),
                    //color_space: s.color_space,
                    //color_transfer: s.color_transfer,
                    //color_primaries: s.color_primaries,
                    //nal_length_size: s.nal_length_size,
                    type_: s.codec_type.as_deref().and_then(|t| match t {
                        "audio" => Some(jellyfin::MediaStreamType::Audio),
                        "video" => Some(jellyfin::MediaStreamType::Video),
                        "subtitle" => Some(jellyfin::MediaStreamType::Subtitle),
                        _ => None,
                    }),
                    ..Default::default()
                }
            })
            .collect();

        let filename = probe.format.filename.clone();
        //let id_encoded = Some(URL_SAFE.encode(&filename));

        jellyfin::MediaSourceInfo {
            // id: id_encoded.clone(),
            //e_tag: id_encoded,
            name: Some(filename),
            media_streams: streams,
            supports_direct_play: Some(true),
            supports_direct_stream: Some(true),
            size: probe.format.size.and_then(|x| x.parse::<i64>().ok()),
            run_time_ticks: probe
                .format
                .duration
                .and_then(|x| x.to_ticks(utils::TickUnit::Seconds)),
            bitrate: probe.format.bit_rate.and_then(|x| x.parse::<i64>().ok()),
            ..Default::default()
        }
    }
}

fn to_option_bool(flag: i64) -> Option<bool> {
    match flag {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}
