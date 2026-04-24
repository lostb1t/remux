pub use remux_sdks::remux::models::*;

use crate::db;
use crate::utils;
use anyhow::Result;

pub fn inject_lyric_stream(source: &mut MediaSourceInfo) {
    let next_idx = source
        .media_streams
        .iter()
        .map(|s| s.index)
        .max()
        .unwrap_or(-1)
        + 1;
    source.media_streams.push(MediaStream {
        type_: Some(MediaStreamType::Lyric),
        index: next_idx,
        is_external: true,
        ..Default::default()
    });
}

pub trait MediaSourceInfoExt {
    fn probe(&self) -> Result<MediaSourceInfo>;
    fn probe_with_url(&self, url: &str) -> Result<MediaSourceInfo>;
}

impl MediaSourceInfoExt for db::Media {
    fn probe(&self) -> Result<MediaSourceInfo> {
        let url = self
            .url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing url"))?;
        self.probe_with_url(url)
    }

    fn probe_with_url(&self, url: &str) -> Result<MediaSourceInfo> {
        let mut probed = crate::transcode::probing::probe_media(url)?;

        probed.id = self.id.clone();
        probed.name = Some(self.title.clone());
        probed.path = self.url.clone();

        Ok(probed)
    }
}

pub fn device_info_from(device: &db::auth::Device) -> DeviceInfo {
    DeviceInfo {
        name: Some(device.name.clone()),
        custom_name: None,
        access_token: Some(device.access_token.clone()),
        id: Some(device.id.clone()),
        last_user_name: None,
        app_name: Some(device.app_name.clone()),
        app_version: Some(device.app_version.clone()),
        last_user_id: Some(device.user_id),
        date_last_activity: device.last_activity_at,
        icon_url: None,
    }
}

pub fn db_display_prefs_to_dto(
    prefs: db::JellyfinDisplayPrefs,
) -> DisplayPreferencesDto {
    let data = prefs.data;
    DisplayPreferencesDto {
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

pub fn db_media_kind_to_type(kind: db::MediaKind) -> MediaType {
    match kind {
        db::MediaKind::Movie => MediaType::Movie,
        db::MediaKind::Series => MediaType::Series,
        db::MediaKind::Season => MediaType::Season,
        db::MediaKind::Episode => MediaType::Episode,
        db::MediaKind::Collection => MediaType::BoxSet,
        db::MediaKind::Genre => MediaType::Genre,
        db::MediaKind::TvChannel => MediaType::TvChannel,
        db::MediaKind::TvProgram => MediaType::Program,
        db::MediaKind::Track => MediaType::Audio,
        db::MediaKind::Album => MediaType::MusicAlbum,
        db::MediaKind::Artist => MediaType::MusicArtist,
        db::MediaKind::Catalog => MediaType::Catalog,
        _ => MediaType::Unknown,
    }
}

pub fn db_media_kind_to_collection_type(
    kind: db::CollectionMediaKind,
) -> CollectionType {
    match kind {
        db::CollectionMediaKind::Movie => CollectionType::Movies,
        db::CollectionMediaKind::Series => CollectionType::Tvshows,
        db::CollectionMediaKind::Music => CollectionType::Music,
    }
}

pub fn db_user_to_dto(user: db::User) -> UserDto {
    let config = user.configuration.map(|c| c.0).unwrap_or_default();
    let mut policy = user.policy.map(|p| p.0).unwrap_or_default();
    policy.is_administrator = user.is_admin;
    let defaults = UserPolicy::default();
    macro_rules! default_if_empty {
        ($field:ident) => {
            if policy.$field.is_empty() {
                policy.$field = defaults.$field.clone();
            }
        };
    }
    default_if_empty!(authentication_provider_id);
    default_if_empty!(password_reset_provider_id);
    UserDto {
        server_id: utils::server_id(),
        name: user.username,
        id: user.id,
        configuration: Some(config),
        policy,
        ..Default::default()
    }
}

pub fn db_state_to_dto(
    state: db::UserMediaState,
    media: &db::Media,
) -> UserItemDataDto {
    let played_percentage = media
        .runtime
        .filter(|&r| r > 0)
        .map(|r| (state.playback_position as f32 / r as f32 * 100.0).clamp(0.0, 100.0));
    UserItemDataDto {
        played: state.played_at.is_some(),
        last_played_date: state
            .last_played_at
            .or(state.played_at)
            .map(|x| x.and_utc()),
        playback_position_ticks: state.playback_position * 10_000_000,
        play_count: state.play_count as i32,
        is_favorite: state.favorite,
        played_percentage,
        unplayed_item_count: media.unplayed_item_count,
        key: state.media_key,
        item_id: media.id,
        ..Default::default()
    }
}

pub fn db_media_to_item(media: db::Media) -> BaseItemDto {
    use crate::utils::IntoVec;
    use crate::utils::ToRunTimeTicks;

    let type_ = match media.kind.clone() {
        db::MediaKind::Movie => MediaType::Movie,
        db::MediaKind::Series => MediaType::Series,
        db::MediaKind::Season => MediaType::Season,
        db::MediaKind::Episode => MediaType::Episode,
        db::MediaKind::Collection => MediaType::BoxSet,
        db::MediaKind::Genre => MediaType::Genre,
        db::MediaKind::TvChannel => MediaType::TvChannel,
        db::MediaKind::TvProgram => MediaType::Program,
        db::MediaKind::Track => MediaType::Audio,
        db::MediaKind::Album => MediaType::MusicAlbum,
        db::MediaKind::Artist => MediaType::MusicArtist,
        _ => MediaType::Unknown,
    };

    let mut item = BaseItemDto {
        id: media.id.clone(),
        etag: Some(media.id),
        server_id: utils::server_id(),
        name: Some(media.title.clone()),
        original_title: Some(media.title.clone()),
        overview: media.description.clone(),
        play_access: Some("Full".to_string()),
        has_lyrics: (media.kind == db::MediaKind::Track).then_some(true),

        type_,
        parent_id: media.parent_id.clone(),
        remote_trailers: media.trailers.clone().map(|j| {
            j.0.into_iter()
                .map(|id| ExternalUrl {
                    name: Some("YouTube".to_string()),
                    url: Some(format!("https://www.youtube.com/watch?v={id}")),
                })
                .collect()
        }),
        series_id: matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Season)
            .then(|| media.series_id.or(media.parent_id))
            .flatten(),
        season_id: (media.kind == db::MediaKind::Episode)
            .then(|| media.parent_id)
            .flatten(),
        user_data: Some(
            media
                .user_state
                .clone()
                .map(|s| db_state_to_dto(s, &media))
                .unwrap_or_else(|| UserItemDataDto {
                    item_id: media.id.clone(),
                    key: media.id.to_string(),
                    unplayed_item_count: media.unplayed_item_count,
                    ..Default::default()
                }),
        ),
        media_type: match media.kind {
            db::MediaKind::Movie
            | db::MediaKind::Episode
            | db::MediaKind::TvChannel
            | db::MediaKind::TvProgram => MediaType::Video,
            db::MediaKind::Track => MediaType::Audio,
            _ => MediaType::Unknown,
        },
        is_movie: Some(media.kind == db::MediaKind::Movie),
        is_series: Some(media.kind == db::MediaKind::Series),
        is_place_holder: media.sources.as_ref().map(|sources| sources.is_empty()),
        premiere_date: media.released_at.clone().map(|d| d.and_utc()),
        digital_release_date: media.digital_released_at.map(|d| d.and_utc()),
        community_rating: media.rating_audience.clone(),
        critic_rating: media.rating_critic.clone(),
        official_rating: media.certification.clone(),
        parent_index_number: media.parent_idx,
        image_tags: Some(ImageTags {
            primary: media.poster.clone(),
            logo: media.logo.clone(),
            backdrop: media.backdrop.clone(),
            ..Default::default()
        }),
        index_number: media.idx,
        is_folder: matches!(
            media.kind,
            db::MediaKind::Series
                | db::MediaKind::Collection
                | db::MediaKind::Season
                | db::MediaKind::Folder
                | db::MediaKind::Album
                | db::MediaKind::Artist
        ),
        channel_type: if matches!(
            media.kind,
            db::MediaKind::TvChannel | db::MediaKind::TvProgram
        ) {
            Some("TV".to_string())
        } else {
            None
        },
        channel_number: media.channel_number.map(|n| n.to_string()),
        start_date: media.live_start.map(|d| d.and_utc().to_rfc3339()),
        end_date: media.live_end.map(|d| d.and_utc().to_rfc3339()),
        is_live: if media.kind == db::MediaKind::TvChannel {
            Some(true)
        } else {
            None
        },
        backdrop_image_tags: media.backdrop.clone().map(|url| vec![url]),
        provider_ids: Some(ProviderIds {
            imdb: media.external_ids.imdb.clone(),
            tmdb: media.external_ids.tmdb.map(|id| id.to_string()),
            tvdb: media.external_ids.tvdb.map(|id| id.to_string()),
            aio: media.media_id.clone(),
            ..Default::default()
        }),
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
                .map(|(_, m)| NameIdPair {
                    id: m.id,
                    name: m.title.clone(),
                })
                .collect()
        }),
        people: media.relations.as_ref().map(|rels| {
            rels.iter()
                .filter(|(_, m)| m.kind == db::MediaKind::Person)
                .map(|(rel, m)| BaseItemPerson {
                    id: m.id,
                    name: m.title.clone(),
                    role: rel.role.as_ref().and_then(|r| match r {
                        db::RelationRole::Actor => {
                            rel.character.clone().or_else(|| Some("Actor".to_string()))
                        }
                        db::RelationRole::Director => Some("Director".to_string()),
                        db::RelationRole::Writer => Some("Writer".to_string()),
                        db::RelationRole::Producer => Some("Producer".to_string()),
                        db::RelationRole::Creator => Some("Creator".to_string()),
                        db::RelationRole::Catalog => None,
                    }),
                    type_: rel.role.as_ref().and_then(|r| match r {
                        db::RelationRole::Actor => Some("Actor".to_string()),
                        db::RelationRole::Director => Some("Director".to_string()),
                        db::RelationRole::Writer => Some("Writer".to_string()),
                        db::RelationRole::Producer => Some("Producer".to_string()),
                        db::RelationRole::Creator => Some("Creator".to_string()),
                        db::RelationRole::Catalog => None,
                    }),
                    primary_image_tag: m.poster.clone(),
                })
                .collect()
        }),
        studios: media.relations.as_ref().map(|rels| {
            rels.iter()
                .filter(|(_, m)| m.kind == db::MediaKind::Studio)
                .map(|(_, m)| NameIdPair {
                    id: m.id,
                    name: m.title.clone(),
                })
                .collect()
        }),
        child_count: media.child_count,
        album_count: media.album_count,
        song_count: media.song_count,
        // Music track fields: album name, album id, artist name
        album: (media.kind == db::MediaKind::Track)
            .then(|| media.parent_title.clone())
            .flatten(),
        album_id: (media.kind == db::MediaKind::Track)
            .then(|| media.parent_id.map(|id| id.to_string()))
            .flatten(),
        album_primary_image_tag: (media.kind == db::MediaKind::Track)
            .then(|| media.poster.clone())
            .flatten(),
        album_artist: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| media.series_title.clone())
            .flatten(),
        album_artists: matches!(
            media.kind,
            db::MediaKind::Track | db::MediaKind::Album
        )
        .then(|| {
            media
                .series_id
                .zip(media.series_title.clone())
                .map(|(id, name)| vec![NameIdPair { id, name }])
        })
        .flatten(),
        artists: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| media.series_title.clone().map(|name| vec![name]))
            .flatten(),
        artist_items: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| {
                media
                    .series_id
                    .zip(media.series_title.clone())
                    .map(|(id, name)| vec![NameIdPair { id, name }])
            })
            .flatten(),
        tags: if media.tags.is_empty() {
            None
        } else {
            Some(media.tags.clone())
        },
        status: media.status.as_ref().map(|s| match s {
            db::MediaStatus::Continuing => Status::Continuing,
            db::MediaStatus::Ended => Status::Ended,
            db::MediaStatus::Unreleased => Status::Unreleased,
            db::MediaStatus::Released | db::MediaStatus::Unknown => Status::Released,
        }),
        remux: Some(RemuxInfo {
            collection_kind: media
                .collection_kind
                .as_ref()
                .and_then(|k| k.to_string().parse().ok()),
            collection_media_kind: media
                .collection_media_kind
                .as_ref()
                .and_then(|k| k.to_string().parse().ok()),
            collection_max_items: media.collection_max_items,
            smart_filter: media.parse_smart_filter().cloned(),
            promoted: Some(media.promoted),
        }),
        ..Default::default()
    };

    // For Season items, season_id should be the season's own ID (not the parent series ID)
    if media.kind == db::MediaKind::Season {
        item.season_id = Some(item.id);
    }

    if matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Season) {
        item.series_name = media.series_title.clone();
        item.series_primary_image_tag = media.series_poster.clone();
        // The series item is where backdrop images live.
        let series_uuid = if media.kind == db::MediaKind::Episode {
            media.series_id.or(media.parent_id)
        } else {
            media.parent_id // season's parent is the series
        };
        item.parent_backdrop_item_id = series_uuid.map(|id| id.to_string());
        item.parent_backdrop_image_tags =
            media.series_backdrop.clone().map(|b| vec![b]);
        if media.kind == db::MediaKind::Episode {
            item.season_name = media.parent_title.clone();
        }
    }

    // Build external URLs from provider IDs
    let mut external_urls = Vec::new();
    if let Some(ref imdb_id) = media.external_ids.imdb {
        external_urls.push(ExternalUrl {
            name: Some("IMDb".to_string()),
            url: Some(format!("https://www.imdb.com/title/{imdb_id}")),
        });
    }
    if !external_urls.is_empty() {
        item.external_urls = Some(external_urls);
    }

    if media.kind == db::MediaKind::Movie
        || media.kind == db::MediaKind::Episode
        || media.kind == db::MediaKind::Track
    {
        item.media_sources = match media.sources.clone() {
            Some(sources) if sources.is_empty() => Some(vec![media.clone().into()]),
            Some(sources) => {
                let mut infos: Vec<MediaSourceInfo> =
                    sources.into_iter().map(MediaSourceInfo::from).collect();
                // Clients expect the first source's ID to equal the parent item's ID.
                if !infos.is_empty() {
                    infos[0].id = media.id;
                    infos[0].e_tag = media.id;
                }
                Some(infos)
            }
            None => None,
        };
    }

    if media.kind == db::MediaKind::TvChannel {
        // Channels use direct-play passthrough — no GStreamer probe needed.
        item.media_sources = Some(vec![MediaSourceInfo {
            id: media.id,
            name: Some(media.title.clone()),
            path: media.url.clone(),
            protocol: "Http".to_string(),
            is_remote: true,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: false,
            ..Default::default()
        }]);
    }

    if media.kind == db::MediaKind::Collection {
        item.collection_type = Some(
            media
                .collection_media_kind
                .clone()
                .map(db_media_kind_to_collection_type)
                .unwrap_or(CollectionType::Unknown),
        );
        item.collection_kind = media.collection_kind.as_ref().map(|k| k.to_string());
        if media.promoted {
            item.type_ = MediaType::CollectionFolder;
            item.display_preferences_id = Some(item.id.to_string());
        } else {
            item.type_ = MediaType::BoxSet;
        }
    }

    if media.kind == db::MediaKind::Folder {
        item.collection_type = Some(CollectionType::Boxsets);
        item.type_ = MediaType::CollectionFolder;
        item.display_preferences_id = Some(item.id.to_string());
    }

    item
}
