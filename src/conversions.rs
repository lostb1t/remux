// src/conversions.rs
use crate::db;
use crate::imdb;
use crate::sdks::{jellyfin, stremio, tmdb};
use crate::utils;
use crate::utils::ToRunTimeTicks;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use eyre::{Result, eyre};
use isolang::Language;
use std::collections::HashMap;
use std::str::FromStr;

use std::convert::{TryFrom, TryInto};

impl TryFrom<imdb::TitleBasics> for db::media::Model {
    type Error = &'static str;

    fn try_from(item: imdb::TitleBasics) -> Result<Self, Self::Error> {
        let media_type = match item.title_type.as_str() {
            "movie" => db::media::MediaType::Movie,
            "short" => db::media::MediaType::Movie, // map shorts as movies?
            "tvSeries" => db::media::MediaType::Series,
            "tvMiniSeries" => db::media::MediaType::Series,
            "tvMovie" => db::media::MediaType::Movie,
            "tvEpisode" => return Err("episode"), // skip episodes
            "tvSpecial" => db::media::MediaType::Movie,
            "video" => db::media::MediaType::Movie,
            "videoGame" => return Err("game"), // or map if you want
            _ => return Err("unlown"),
        };

        Ok(Self {
            id: item.tconst,
            name: item.primary_title,
            media_type,
            ..Default::default()
        })
    }
}

impl From<tmdb::Movie> for jellyfin::BaseItemDto {
    fn from(item: tmdb::Movie) -> Self {
        Self {
            id: Some(item.id.to_string()),
            name: Some(item.title),
            type_: Some(jellyfin::MediaType::Movie),
            ..Default::default()
        }
    }
}

impl From<tmdb::Season> for jellyfin::BaseItemDto {
    fn from(item: tmdb::Season) -> Self {
        Self {
            id: Some(item.id.to_string()),
            index_number: Some(item.season_number as i32),
            name: Some(item.name),
            //parent_id: Some("92053".to_string()),
            type_: Some(jellyfin::MediaType::Season),
            ..Default::default()
        }
    }
}

impl From<tmdb::Episode> for jellyfin::BaseItemDto {
    fn from(item: tmdb::Episode) -> Self {
        Self {
            id: Some(item.id.to_string()),
            name: Some(item.name),
            type_: Some(jellyfin::MediaType::Episode),
            ..Default::default()
        }
    }
}

impl From<tmdb::Movie> for db::media::Model {
    fn from(item: tmdb::Movie) -> Self {
        Self {
            tmdb_id: Some(item.id),
            //  id: item.id,
            //imdb_id: item.imdb_id,
            community_rating: item.vote_average,
            release_date: item.release_date,
            status: item.status.map(|x| x.into()),
            // imdb_id: item.external_ids.and_then(|ids| ids.imdb_id),
            id: item.external_ids.and_then(|ids| ids.imdb_id).unwrap(),
            name: item.title,
            overview: item.overview,
            runtime: item.runtime,
            poster_path: item.poster_path,
            backdrop_path: item.backdrop_path,
            media_type: db::media::MediaType::Movie,
            ..Default::default()
        }
    }
}

impl From<tmdb::Series> for db::media::Model {
    fn from(item: tmdb::Series) -> Self {
        Self {
            tmdb_id: Some(item.id),
            release_date: item.first_air_date,
            // imdb_id: item.external_ids.and_then(|ids| ids.imdb_id),
            id: item.external_ids.and_then(|ids| ids.imdb_id).unwrap(),
            name: item.name,
            community_rating: item.vote_average,
            overview: item.overview,
            status: item.status.map(|x| x.into()),
            poster_path: item.poster_path,
            backdrop_path: item.backdrop_path,
            media_type: db::media::MediaType::Series,
            genres: item.genres.map(|items| {
                items
                    .into_iter()
                    .filter_map(|g| match db::Genre::try_from(g) {
                        Ok(genre) => Some(genre),
                        Err(err) => {
                            tracing::warn!("{:?}", err);
                            None
                        }
                    })
                    .collect()
            }),
            ..Default::default()
        }
    }
}

impl From<tmdb::Season> for db::media::Model {
    fn from(item: tmdb::Season) -> Self {
        Self {
            tmdb_id: Some(item.id),
            release_date: item.air_date,
            name: item.name,
            overview: item.overview,
            index_number: Some(item.season_number),
            community_rating: item.vote_average,
            poster_path: item.poster_path,
            //backdrop_path: item.backdrop_path,
            media_type: db::media::MediaType::Season,
            ..Default::default()
        }
    }
}

impl From<tmdb::Episode> for db::media::Model {
    fn from(item: tmdb::Episode) -> Self {
        Self {
            tmdb_id: Some(item.id),
            release_date: item.air_date,
            name: item.name,
            overview: item.overview,
            community_rating: item.vote_average,
            index_number: Some(item.episode_number),
            parent_index_number: Some(item.season_number),
            runtime: item.runtime,
            poster_path: item.still_path,
            //backdrop_path: item.backdrop_path,
            media_type: db::media::MediaType::Episode,
            ..Default::default()
        }
    }
}

impl From<stremio::Meta> for jellyfin::BaseItemDto {
    fn from(meta: stremio::Meta) -> Self {
        // dbg!(&meta);
        let media_type: jellyfin::MediaType = meta.media_type.into();

        jellyfin::BaseItemDto {
            id: Some(utils::encode_media_uuid(&meta.imdb_id.unwrap(), media_type)),
            name: meta.name.clone(),
            overview: meta.description.clone(),
            type_: Some(media_type),
            //premiere_date: meta.released.and_then(utils::native_to_utc),
            community_rating: meta.imdb_rating.and_then(|r| r.parse().ok()),
            image_tags: meta.poster.map(|p| jellyfin::ImageTags {
                primary: Some(p),
                ..Default::default()
            }),
            genres: meta.genres,
            // runtime: meta.year.and_then(|y| y.parse().ok()).and_then(|mins| {
            //     (mins as i32).to_ticks(utils::TickUnit::Minutes)
            // }),
            ..Default::default()
        }
    }
}

//Resources
impl From<stremio::Stream> for jellyfin::MediaSourceInfo {
    fn from(item: stremio::Stream) -> Self {
        let id = Some(URL_SAFE.encode(item.url.unwrap()));

        //let streams =

        Self {
            // base64 encode url
            //id: Some("yoo".to_string()),
            id: id.clone(),
            e_tag: id,
            name: Some(item.name.unwrap()),

            supports_direct_play: Some(true),
            supports_direct_stream: Some(true),
            ..Default::default()
        }
    }
}

impl From<stremio::Catalog> for jellyfin::BaseItemDto {
    fn from(item: stremio::Catalog) -> Self {
        jellyfin::BaseItemDto {
            name: Some(item.name.clone()),
            id: Some(item.uuid),
            type_: Some(jellyfin::MediaType::BoxSet),
            ..Default::default()
        }
    }
}

impl From<db::media::Model> for jellyfin::BaseItemDto {
    fn from(media: db::media::Model) -> Self {
        jellyfin::BaseItemDto {
            name: Some(media.name),
            overview: media.overview,
            id: Some(media.id.to_string()),
            //type_: Some(media.media_type.into()),
            premiere_date: utils::native_to_utc(media.release_date),
            run_time_ticks: media
                .runtime
                .and_then(|x| x.to_ticks(utils::TickUnit::Minutes)),
            image_tags: Some(jellyfin::ImageTags {
                primary: media.poster_path,
                ..Default::default()
            }),
            community_rating: media.community_rating,
            parent_id: media.parent_id.map(|x| x.to_string()),
            backdrop_image_tags: media.backdrop_path.map(|path| vec![path]),
            media_sources: media.streams.as_ref().map(|r| {
                // r.streams
                r.clone()
                    // .unwrap_or_default()
                    .into_iter()
                    .map(|stream| stream.into_media_source())
                    .collect()
            }),
            // TODO!!! this for some reason breaks swiftfin listings
            //provider_ids: Some(HashMap::from([
            //   ("Tmdb".to_string(), media.tmdb_id.map(|v| v.to_string())),
            //   ("Imdb".to_string(), media.imdb_id),
            //])),
            ..Default::default()
        }
    }
}

//impl From<db::media::MediaType> for jellyfin::BaseItemKind {
//    fn from(media: db::media::MediaType) -> Self {
//        jellyfin::BaseItemKind::Movie
//    }
//}

impl From<stremio::MediaType> for jellyfin::MediaType {
    fn from(kind: stremio::MediaType) -> Self {
        match kind {
            stremio::MediaType::Movie => jellyfin::MediaType::Movie,
            stremio::MediaType::Series => jellyfin::MediaType::Series,
            _ => jellyfin::MediaType::Unknown,
        }
    }
}

impl From<jellyfin::MediaType> for stremio::MediaType {
    fn from(kind: jellyfin::MediaType) -> Self {
        match kind {
            jellyfin::MediaType::Movie => stremio::MediaType::Movie,
            jellyfin::MediaType::Series => stremio::MediaType::Series,
            _ => stremio::MediaType::Unknown,
        }
    }
}

impl From<db::media::MediaType> for stremio::MediaType {
    fn from(kind: db::media::MediaType) -> Self {
        match kind {
            db::media::MediaType::Movie => stremio::MediaType::Movie,
            db::media::MediaType::Series => stremio::MediaType::Series,
            db::media::MediaType::Episode => stremio::MediaType::Series,
            _ => stremio::MediaType::Movie,
        }
    }
}

impl From<stremio::MediaType> for db::media::MediaType {
    fn from(kind: stremio::MediaType) -> Self {
        match kind {
            stremio::MediaType::Movie => db::media::MediaType::Movie,
            stremio::MediaType::Series => db::media::MediaType::Series,
            _ => db::media::MediaType::Movie,
        }
    }
}

impl From<tmdb::Status> for db::media::Status {
    fn from(kind: tmdb::Status) -> Self {
        db::media::Status::from_str(&kind.to_string()).unwrap()
    }
}

impl TryFrom<tmdb::Genre> for db::Genre {
    type Error = eyre::Report;

    fn try_from(item: tmdb::Genre) -> Result<Self> {
        db::Genre::from_str(item.name.as_str())
            .map_err(|_| eyre!("Unknown genre: {}", item.name))
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
                    bit_rate: s.bit_rate.map(|x| x.parse::<i32>().unwrap()),
                    codec: s.codec_name.clone(),
                    codec_tag: Some(s.codec_tag),
                    //codec_time_base: s.codec_time_base,
                    height: s.height.map(|x| x as i32),
                    width: s.width.map(|x| x as i32),
                    channels: s.channels.map(|x| x as i32),
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
                    index: Some(s.index as i32),
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
            media_streams: Some(streams),
            supports_direct_play: Some(true),
            supports_direct_stream: Some(true),
            size: probe.format.size.and_then(|x| x.parse::<i64>().ok()),
            run_time_ticks: probe
                .format
                .duration
                .and_then(|x| x.to_ticks(utils::TickUnit::Seconds)),
            bitrate: probe.format.bit_rate.and_then(|x| x.parse::<i32>().ok()),
            ..Default::default()
        }
    }
}

impl From<jellyfin::SortOrder> for sea_orm::Order {
    fn from(order: jellyfin::SortOrder) -> Self {
        match order {
            jellyfin::SortOrder::Ascending => sea_orm::Order::Asc,
            jellyfin::SortOrder::Descending => sea_orm::Order::Desc,
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
