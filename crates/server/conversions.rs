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
            scroll_direction: data.scroll_direction.parse().ok(),
            show_backdrop: data.show_backdrop,
            remember_sorting: data.remember_sorting,
            sort_order: data.sort_order.parse().ok(),
            show_sidebar: data.show_sidebar,
            client: prefs.client,
        }
    }
}

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
            scroll_direction: dto.scroll_direction.map(|d| d.to_string()).unwrap_or_else(|| "Horizontal".to_string()),
            show_backdrop: dto.show_backdrop,
            remember_sorting: dto.remember_sorting,
            sort_order: dto.sort_order.map(|s| s.to_string()).unwrap_or_else(|| "Ascending".to_string()),
            show_sidebar: dto.show_sidebar,
            home_sections: None,
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
            user_data: media.user_state.clone().map(|state| state.into()),
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
            media_type: {
                match media.kind {
                    db::MediaKind::Movie | db::MediaKind::Episode => {
                        "Video".to_string()
                    }
                    _ => "Unknown".to_string(),
                }
            },
            is_place_holder: media.sources.as_ref().map(|sources| sources.is_empty()),
            premiere_date: media.released_at.clone().map(|d| d.and_utc()),
            community_rating: media.rating_audience.clone(),
            //critic_rating: media.rating_rt.clone().and_then(|r| r.parse().ok()),
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
                || media.kind == db::MediaKind::Folder
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
                aio: media.aio_id.clone(),
                ..Default::default()
            }),
            //genres: meta.genres.clone(),
            run_time_ticks: media
                .runtime
                .map(|r| r.to_ticks(utils::TickUnit::Seconds).unwrap()),

            genres: media.relations.as_ref().map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                    .map(|(_, m)| m.title.clone())
                    .collect()
            }),
            genre_items: media.relations.as_ref().map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                    .map(|(_, m)| jellyfin::NameIdPair {
                        id: m.id,
                        name: m.title.clone(),
                    })
                    .collect()
            }),
            people: media.relations.as_ref().map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Person)
                    .map(|(rel, m)| jellyfin::BaseItemPerson {
                        id: m.id,
                        name: m.title.clone(),
                        role: rel.role.as_ref().map(|r| match r {
                            db::RelationRole::Actor => "Actor".to_string(),
                            db::RelationRole::Director => "Director".to_string(),
                            db::RelationRole::Writer => "Writer".to_string(),
                        }),
                        type_: rel.role.as_ref().map(|r| match r {
                            db::RelationRole::Actor => "Actor".to_string(),
                            db::RelationRole::Director => "Director".to_string(),
                            db::RelationRole::Writer => "Writer".to_string(),
                        }),
                        primary_image_tag: m.poster.clone(),
                    })
                    .collect()
            }),
            studios: media.relations.as_ref().map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Studio)
                    .map(|(_, m)| jellyfin::NameIdPair {
                        id: m.id,
                        name: m.title.clone(),
                    })
                    .collect()
            }),

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

        if media.kind == db::MediaKind::Catalog {
            item.collection_type = Some(
                media
                    .catalog_media_kind
                    .map(|kind| kind.into())
                    .unwrap_or(jellyfin::CollectionType::Unknown),
            );
            if media.promoted == 1 {
                item.type_ = jellyfin::MediaType::CollectionFolder;
            } else {
                item.type_ = jellyfin::MediaType::BoxSet;
            }
        }

        // soecial case (collections)
        if media.kind == db::MediaKind::Folder {
            item.collection_type = Some(jellyfin::CollectionType::Boxsets);
            item.type_ = jellyfin::MediaType::CollectionFolder;
        }

        item
    }
}

impl From<db::MediaKind> for jellyfin::CollectionType {
    fn from(kind: db::MediaKind) -> Self {
        match kind {
            db::MediaKind::Movie => jellyfin::CollectionType::Movies,
            db::MediaKind::Series => jellyfin::CollectionType::Tvshows,
            //db::MediaKind::Catalog => jellyfin::CollectionType::Boxsets,
            _ => jellyfin::CollectionType::Unknown,
        }
    }
}

//impl From<aio::Episode> for jellyfin::BaseItemDto {
// fn from(item: aio::Episode) -> Self {
impl TryFrom<aio::Episode> for db::Media {
    type Error = anyhow::Error;
    fn try_from(meta: aio::Episode) -> Result<db::Media> {
        Ok(db::Media {
            title: meta.title.unwrap_or_default(),
            kind: db::MediaKind::Episode,
            released_at: meta.released.map(|x| x.naive_utc()),
            runtime: meta.runtime.map(|d| d.num_seconds()),
            //  rating_audience: meta.imdb_rating,
            description: meta.overview,
            // certification: meta.certification,
            poster: meta.thumbnail,

            //imdb_id: meta.imdb_id.clone(),
            //  aio_id: meta.imdb_id.clone(),

            //tmdb_id: Some(imdb_id.clone()),
            ..Default::default()
        })
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
        let config = user.configuration.map(|c| c.0).unwrap_or_default();
        let mut policy = user.policy.map(|p| p.0).unwrap_or_default();
        policy.is_administrator = user.is_admin;
        // Replace empty strings with proper defaults for fields that clients
        // decode as strict enums or require non-empty values.
        let defaults = jellyfin::UserPolicy::default();
        macro_rules! default_if_empty {
            ($field:ident) => {
                if policy.$field.as_deref() == Some("") {
                    policy.$field = defaults.$field;
                }
            };
        }
        default_if_empty!(authentication_provider_id);
        default_if_empty!(password_reset_provider_id);
        jellyfin::UserDto {
            server_id: server_id(),
            name: user.username,
            id: user.id,
            configuration: Some(config),
            policy,
            ..Default::default()
        }
    }
}
impl From<db::UserMediaState> for jellyfin::UserItemDataDto {
    fn from(state: db::UserMediaState) -> Self {
        jellyfin::UserItemDataDto {
            played: Some(state.played_at.is_some()),
            last_played_date: state.played_at.map(|x| x.and_utc()),
            playback_position_ticks: Some(state.playback_position * 10_000), // Convert seconds to ticks (1 tick = 100 nanoseconds)
            play_count: Some(state.play_count as i32),
            is_favorite: Some(state.favorite),
            key: Some(state.media_key),
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
            _ => todo!(),
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
