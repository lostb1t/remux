pub use remux_sdks::remux::*;

use crate::common;
use crate::db;
use anyhow::Result;
use chrono::Datelike;

pub fn inject_lyric_stream(source: &mut MediaSourceInfo) {
    let next_idx = source
        .media_streams
        .iter()
        .map(|s| s.index)
        .max()
        .unwrap_or(-1)
        + 1;
    source
        .media_streams
        .push(MediaStream {
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
        use crate::stream::StreamDescriptor;
        let url = match self
            .stream_info
            .as_ref()
            .map(|si| &si.descriptor)
        {
            Some(StreamDescriptor::Http { url, .. }) => url.clone(),
            Some(StreamDescriptor::Local(p)) => p
                .to_string_lossy()
                .into_owned(),
            _ => return Err(anyhow::anyhow!("cannot probe this stream type directly")),
        };
        self.probe_with_url(&url)
    }

    fn probe_with_url(&self, url: &str) -> Result<MediaSourceInfo> {
        let (mut probed, _) = crate::transcode::probing::probe_media(url)?;

        probed.id = self
            .id
            .clone();
        probed.name = Some(
            self.title
                .clone(),
        );
        probed.path = self
            .stream_info
            .as_ref()
            .and_then(|si| {
                si.descriptor
                    .as_http_url()
                    .map(str::to_owned)
            });

        Ok(probed)
    }
}

pub fn device_info_from(device: &db::auth::Device) -> DeviceInfo {
    DeviceInfo {
        name: Some(
            device
                .name
                .clone(),
        ),
        custom_name: None,
        access_token: Some(
            device
                .access_token
                .clone(),
        ),
        id: Some(
            device
                .id
                .clone(),
        ),
        last_user_name: None,
        app_name: Some(
            device
                .app_name
                .clone(),
        ),
        app_version: Some(
            device
                .app_version
                .clone(),
        ),
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
        view_type: data
            .view_type
            .clone(),
        sort_by: data
            .sort_by
            .clone(),
        index_by: data
            .index_by
            .clone(),
        remember_indexing: data.remember_indexing,
        primary_image_height: data.primary_image_height,
        primary_image_width: data.primary_image_width,
        custom_prefs: data
            .custom_prefs
            .clone(),
        scroll_direction: data
            .scroll_direction
            .clone(),
        show_backdrop: data.show_backdrop,
        remember_sorting: data.remember_sorting,
        sort_order: data
            .sort_order
            .clone(),
        show_sidebar: data.show_sidebar,
        client: prefs.client,
    }
}

impl Into<MediaType> for db::MediaKind {
    fn into(self) -> MediaType {
        match self {
            db::MediaKind::Movie => MediaType::Movie,
            db::MediaKind::Series => MediaType::Series,
            db::MediaKind::Season => MediaType::Season,
            db::MediaKind::Episode => MediaType::Episode,
            db::MediaKind::Collection => MediaType::BoxSet,
            db::MediaKind::Folder => MediaType::Folder,
            db::MediaKind::Genre => MediaType::Genre,
            db::MediaKind::Person => MediaType::Person,
            db::MediaKind::Studio => MediaType::Studio,
            db::MediaKind::TvChannel => MediaType::TvChannel,
            db::MediaKind::TvProgram => MediaType::Program,
            db::MediaKind::Track => MediaType::Audio,
            db::MediaKind::Album => MediaType::MusicAlbum,
            db::MediaKind::Artist => MediaType::MusicArtist,
            db::MediaKind::Playlist => MediaType::Playlist,
            db::MediaKind::Stream | db::MediaKind::StreamGroup => MediaType::Video,
        }
    }
}

pub fn db_media_kind_to_collection_type(
    kind: db::CollectionMediaKind,
) -> CollectionType {
    match kind {
        db::CollectionMediaKind::Movie => CollectionType::Movies,
        db::CollectionMediaKind::Series => CollectionType::Tvshows,
        db::CollectionMediaKind::Music => CollectionType::Music,
        db::CollectionMediaKind::Collection => CollectionType::Boxsets,
        db::CollectionMediaKind::Playlist => CollectionType::Playlists,
    }
}

pub fn db_user_to_dto(data_dir: &std::path::Path, user: db::User) -> UserDto {
    let config = user
        .configuration
        .map(|c| c.0)
        .unwrap_or_default();
    let mut policy = user
        .policy
        .map(|p| p.0)
        .unwrap_or_default();
    policy.is_administrator = user.is_admin;
    let defaults = UserPolicy::default();
    macro_rules! default_if_empty {
        ($field:ident) => {
            if policy
                .$field
                .is_empty()
            {
                policy.$field = defaults
                    .$field
                    .clone();
            }
        };
    }
    default_if_empty!(authentication_provider_id);
    default_if_empty!(password_reset_provider_id);
    let primary_image_tag = if crate::api::users::user_has_avatar(data_dir, &user.id) {
        Some(
            user.id
                .to_string(),
        )
    } else {
        None
    };
    UserDto {
        server_id: common::server_id(),
        name: user.username,
        id: user.id,
        configuration: Some(config),
        policy,
        primary_image_tag,
        ..Default::default()
    }
}

fn image_tag(url: Option<&str>) -> Option<String> {
    url.map(|u| u.to_string())
}

fn media_image_tag(media: &db::Media, kind: db::ImageKind) -> Option<String> {
    media
        .images
        .get(kind)
        .map(|i| {
            i.id.to_string()
        })
}

fn parent_image_tag(parent: Option<&db::Media>, kind: db::ImageKind) -> Option<String> {
    parent?
        .images
        .get(kind)
        .map(|i| {
            i.id.to_string()
        })
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
        played: state
            .played_at
            .is_some(),
        last_played_date: state
            .last_played_at
            .or(state.played_at)
            .map(|x| x.and_utc()),
        playback_position_ticks: state.playback_position * 10_000_000,
        play_count: state.play_count as i32,
        is_favorite: state.favorite,
        played_percentage,
        unplayed_item_count: media.unplayed_item_count,
        key: state
            .media_raw
            .unwrap_or_default(),
        item_id: media.id,
        ..Default::default()
    }
}

pub fn db_media_to_item(media: db::Media) -> BaseItemDto {
    use crate::common::IntoVec;
    use crate::common::ToRunTimeTicks;

    let type_ = media
        .kind
        .clone()
        .into();

    let mut item = BaseItemDto {
        id: media
            .id
            .clone(),
        etag: Some(media.id),
        server_id: common::server_id(),
        name: Some(
            media
                .title
                .clone(),
        ),
        overview: media
            .description
            .clone(),
        play_access: matches!(
            media.kind,
            db::MediaKind::Movie
                | db::MediaKind::Episode
                | db::MediaKind::Track
                | db::MediaKind::TvChannel
                | db::MediaKind::TvProgram
        )
        .then(|| "Full".to_string()),
        has_lyrics: (media.kind == db::MediaKind::Track).then_some(true),
        type_,
        parent_id: media
            .parent_id
            .clone(),
        remote_trailers: media
            .trailers
            .clone()
            .map(|j| {
                j.into_iter()
                    .map(|id| ExternalUrl {
                        name: Some("YouTube".to_string()),
                        url: Some(format!("https://www.youtube.com/watch?v={id}")),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        series_id: matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Season)
            .then(|| {
                media
                    .grandparent_id
                    .or(media.parent_id)
            })
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
                    item_id: media
                        .id
                        .clone(),
                    key: media
                        .id
                        .to_string(),
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
            db::MediaKind::Playlist => match media.collection_media_kind {
                Some(db::CollectionMediaKind::Music) => MediaType::Audio,
                Some(_) => MediaType::Video,
                None => MediaType::Unknown,
            },
            _ => MediaType::Unknown,
        },
        is_movie: (media.kind == db::MediaKind::Movie
            || matches!(media.program_kind, Some(db::ProgramKind::Movie)))
        .then_some(true),
        is_series: (media.kind == db::MediaKind::Series
            || matches!(media.program_kind, Some(db::ProgramKind::Series)))
        .then_some(true),
        is_news: media
            .program_kind
            .as_ref()
            .map(|k| matches!(k, db::ProgramKind::News)),
        is_kids: media
            .program_kind
            .as_ref()
            .map(|k| matches!(k, db::ProgramKind::Kids)),
        is_sports: media
            .program_kind
            .as_ref()
            .map(|k| matches!(k, db::ProgramKind::Sports)),
        is_place_holder: media
            .sources
            .as_ref()
            .map(|sources| sources.is_empty()),
        premiere_date: media
            .released_at
            .clone()
            .map(|d| d.and_utc()),
        production_year: media
            .released_at
            .map(|d| d.year() as i64),
        community_rating: media
            .rating_audience
            .clone(),
        critic_rating: media
            .rating_critic
            .clone(),
        official_rating: media
            .certification
            .clone(),
        parent_index_number: media.parent_idx,
        image_tags: Some(ImageTags {
            primary: media_image_tag(&media, db::ImageKind::Primary).or_else(|| {
                // For collections/folders with no poster image, set a synthetic
                // tag so clients know to request the generated placeholder.
                if matches!(
                    media.kind,
                    db::MediaKind::Collection | db::MediaKind::Folder
                ) {
                    Some(
                        media
                            .id
                            .to_string(),
                    )
                } else {
                    None
                }
            }),
            logo: media_image_tag(&media, db::ImageKind::Logo),
            backdrop: media_image_tag(&media, db::ImageKind::Backdrop),
            ..Default::default()
        }),
        index_number: media.idx,
        is_folder: media
            .kind
            .is_folder(),
        channel_type: if matches!(
            media.kind,
            db::MediaKind::TvChannel | db::MediaKind::TvProgram
        ) {
            Some("TV".to_string())
        } else {
            None
        },
        channel_number: media
            .channel_number
            .or_else(|| {
                media
                    .parent
                    .as_ref()
                    .and_then(|p| p.channel_number)
            })
            .map(|n| n.to_string()),
        start_date: media
            .live_start
            .map(|d| {
                d.and_utc()
                    .to_rfc3339()
            }),
        end_date: media
            .live_end
            .map(|d| {
                d.and_utc()
                    .to_rfc3339()
            }),
        is_live: if media.kind == db::MediaKind::TvChannel {
            Some(true)
        } else {
            None
        },
        backdrop_image_tags: media_image_tag(&media, db::ImageKind::Backdrop)
            .map(|tag| vec![tag])
            .unwrap_or_default(),
        provider_ids: Some(ProviderIds {
            // Episodes ingested before the per-episode external_ids fix
            // have no IMDB id of their own — fall back to the series's
            // IMDB so reviews / remote-image lookups have something to
            // work with on existing data without forcing a re-import.
            imdb: media
                .external_ids
                .imdb
                .clone()
                .or_else(|| {
                    if matches!(
                        media.kind,
                        db::MediaKind::Episode | db::MediaKind::Season
                    ) {
                        media
                            .external_ids
                            .series_imdb
                            .clone()
                    } else {
                        None
                    }
                }),
            tmdb: media
                .external_ids
                .tmdb
                .map(|id| id.to_string()),
            tvdb: media
                .external_ids
                .tvdb
                .map(|id| id.to_string()),
            ..Default::default()
        }),
        run_time_ticks: media
            .runtime
            .map(|r| {
                r.to_ticks(common::TickUnit::Seconds)
                    .unwrap()
            })
            .or_else(|| {
                if let (Some(start), Some(end)) = (media.live_start, media.live_end) {
                    let secs = (end - start).num_seconds();
                    if secs > 0 {
                        Some(secs * 10_000_000)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }),
        genres: media
            .relations
            .as_ref()
            .map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                    .map(|(_, m)| {
                        m.title
                            .clone()
                    })
                    .collect()
            })
            .unwrap_or_default(),
        genre_items: media
            .relations
            .as_ref()
            .map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                    .map(|(_, m)| NameIdPair {
                        id: m.id,
                        name: m
                            .title
                            .clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        people: media
            .relations
            .as_ref()
            .map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Person)
                    .map(|(rel, m)| BaseItemPerson {
                        id: m.id,
                        name: m
                            .title
                            .clone(),
                        role: rel
                            .role
                            .as_ref()
                            .and_then(|r| match r {
                                db::RelationRole::Actor => rel
                                    .character
                                    .clone()
                                    .or_else(|| Some("Actor".to_string())),
                                db::RelationRole::Director => {
                                    Some("Director".to_string())
                                }
                                db::RelationRole::Writer => Some("Writer".to_string()),
                                db::RelationRole::Producer => {
                                    Some("Producer".to_string())
                                }
                                db::RelationRole::Creator => {
                                    Some("Creator".to_string())
                                }
                                db::RelationRole::Catalog
                                | db::RelationRole::Playlist
                                | db::RelationRole::Collection => None,
                            }),
                        type_: rel
                            .role
                            .as_ref()
                            .and_then(|r| match r {
                                db::RelationRole::Actor => Some("Actor".to_string()),
                                db::RelationRole::Director => {
                                    Some("Director".to_string())
                                }
                                db::RelationRole::Writer => Some("Writer".to_string()),
                                db::RelationRole::Producer => {
                                    Some("Producer".to_string())
                                }
                                db::RelationRole::Creator => {
                                    Some("Creator".to_string())
                                }
                                db::RelationRole::Catalog
                                | db::RelationRole::Playlist
                                | db::RelationRole::Collection => None,
                            }),
                        primary_image_tag: media_image_tag(m, db::ImageKind::Primary),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        studios: media
            .relations
            .as_ref()
            .map(|rels| {
                rels.iter()
                    .filter(|(_, m)| m.kind == db::MediaKind::Studio)
                    .map(|(_, m)| NameIdPair {
                        id: m.id,
                        name: m
                            .title
                            .clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        child_count: media.child_count,
        recursive_item_count: media.recursive_item_count,
        album_count: media.album_count,
        song_count: media.song_count,
        movie_count: media.movie_count,
        series_count: media.series_count,
        // Music track fields: album name, album id, artist name
        album: (media.kind == db::MediaKind::Track)
            .then(|| {
                media
                    .parent
                    .as_ref()
                    .map(|p| {
                        p.title
                            .clone()
                    })
            })
            .flatten(),
        album_id: (media.kind == db::MediaKind::Track)
            .then(|| {
                media
                    .parent_id
                    .map(|id| id.to_string())
            })
            .flatten(),
        album_primary_image_tag: (media.kind == db::MediaKind::Track)
            .then(|| media_image_tag(&media, db::ImageKind::Primary))
            .flatten(),
        album_artist: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| {
                media
                    .grandparent
                    .as_ref()
                    .map(|gp| {
                        gp.title
                            .clone()
                    })
            })
            .flatten(),
        album_artists: matches!(
            media.kind,
            db::MediaKind::Track | db::MediaKind::Album
        )
        .then(|| {
            media
                .grandparent_id
                .zip(
                    media
                        .grandparent
                        .as_ref()
                        .map(|gp| {
                            gp.title
                                .clone()
                        }),
                )
                .map(|(id, name)| vec![NameIdPair { id, name }])
        })
        .flatten(),
        artists: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| {
                media
                    .grandparent
                    .as_ref()
                    .map(|gp| {
                        vec![
                            gp.title
                                .clone(),
                        ]
                    })
            })
            .flatten(),
        artist_items: matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            .then(|| {
                media
                    .grandparent_id
                    .zip(
                        media
                            .grandparent
                            .as_ref()
                            .map(|gp| {
                                gp.title
                                    .clone()
                            }),
                    )
                    .map(|(id, name)| vec![NameIdPair { id, name }])
            })
            .flatten(),
        tags: media
            .tags
            .clone(),
        status: media
            .status
            .as_ref()
            .map(|s| match s {
                db::MediaStatus::Continuing => Status::Continuing,
                db::MediaStatus::Ended => Status::Ended,
                db::MediaStatus::Unreleased => Status::Unreleased,
                db::MediaStatus::Released | db::MediaStatus::Unknown => {
                    Status::Released
                }
            }),
        sort_name: Some(
            media
                .title
                .to_ascii_lowercase(),
        ),
        primary_image_aspect_ratio: Some(
            media
                .images
                .get(db::ImageKind::Primary)
                .and_then(|i| {
                    let (w, h) = (i.width?, i.height?);
                    if h == 0 {
                        return None;
                    }
                    Some(w as f32 / h as f32)
                })
                .unwrap_or_else(|| match media.kind {
                    db::MediaKind::Episode
                    | db::MediaKind::Collection
                    | db::MediaKind::Folder => 16.0 / 9.0,
                    _ => 0.6,
                }),
        ),
        remux: Some(RemuxInfo {
            collection_kind: media
                .collection_kind
                .as_ref()
                .and_then(|k| {
                    k.to_string()
                        .parse()
                        .ok()
                }),
            collection_media_kind: media
                .collection_media_kind
                .as_ref()
                .and_then(|k| {
                    k.to_string()
                        .parse()
                        .ok()
                }),
            collection_max_items: media.collection_max_items,
            smart_filter: media
                .parse_smart_filter()
                .cloned(),
            promoted: Some(media.promoted),
            digital_release_date: media
                .digital_released_at
                .map(|d| d.and_utc()),
            latest_auto_unplayed: media.collection_latest_auto_unplayed,
            latest_sort_digital: media.collection_latest_sort_digital,
        }),
        date_created: Some(
            media
                .created_at
                .and_utc()
                .to_rfc3339(),
        ),
        production_locations: (media.kind == db::MediaKind::Person)
            .then(|| {
                media
                    .country
                    .clone()
                    .map(|c| vec![c])
            })
            .flatten(),
        ..Default::default()
    };

    // For Season items, season_id should be the season's own ID (not the parent series ID)
    if media.kind == db::MediaKind::Season {
        item.season_id = Some(item.id);
    }

    if matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Season) {
        item.series_name = media
            .grandparent
            .as_ref()
            .map(|gp| {
                gp.title
                    .clone()
            });
        item.series_primary_image_tag = parent_image_tag(
            media
                .grandparent
                .as_deref(),
            db::ImageKind::Primary,
        );
        // The series item is where backdrop images live.
        let series_uuid = if media.kind == db::MediaKind::Episode {
            media
                .grandparent_id
                .or(media.parent_id)
        } else {
            media.parent_id // season's parent is the series
        };
        item.parent_backdrop_item_id = series_uuid.map(|id| id.to_string());
        item.parent_backdrop_image_tags = parent_image_tag(
            media
                .grandparent
                .as_deref(),
            db::ImageKind::Backdrop,
        )
        .map(|b| vec![b]);
        // Thumb: prefer season (direct parent) when it has a thumb image;
        // fall back to series thumb/backdrop so the field is never empty.
        let season_thumb = (media.kind == db::MediaKind::Episode)
            .then(|| {
                parent_image_tag(
                    media
                        .parent
                        .as_deref(),
                    db::ImageKind::Thumb,
                )
            })
            .flatten();
        if season_thumb.is_some() {
            item.parent_thumb_item_id = media
                .parent_id
                .map(|id| id.to_string());
            item.parent_thumb_image_tag = season_thumb;
        } else {
            item.parent_thumb_item_id = series_uuid.map(|id| id.to_string());
            item.parent_thumb_image_tag = parent_image_tag(
                media
                    .grandparent
                    .as_deref(),
                db::ImageKind::Thumb,
            )
            .or_else(|| {
                parent_image_tag(
                    media
                        .grandparent
                        .as_deref(),
                    db::ImageKind::Backdrop,
                )
            });
        }
        if media.kind == db::MediaKind::Episode {
            item.season_name = media
                .parent
                .as_ref()
                .map(|p| {
                    p.title
                        .clone()
                });
        }
    }

    // Build external URLs from provider IDs
    let mut external_urls = Vec::new();
    if let Some(ref imdb_id) = media
        .external_ids
        .imdb
    {
        external_urls.push(ExternalUrl {
            name: Some("IMDb".to_string()),
            url: Some(format!("https://www.imdb.com/title/{imdb_id}")),
        });
    }
    item.external_urls = external_urls;

    // several dlients require at least one stream
    if media.kind == db::MediaKind::Movie
        || media.kind == db::MediaKind::Episode
        || media.kind == db::MediaKind::Track
    {
        item.media_sources = match media
            .sources
            .clone()
        {
            Some(sources) if sources.is_empty() => Some(vec![
                media
                    .clone()
                    .into(),
            ]),
            Some(sources) => {
                let mut infos: Vec<MediaSourceInfo> = sources
                    .into_iter()
                    .map(MediaSourceInfo::from)
                    .collect();
                // Clients expect the first source's ID to equal the parent item's ID.
                if !infos.is_empty() {
                    infos[0].id = media.id;
                    infos[0].e_tag = media.id;
                }
                Some(infos)
            }
            None => Some(vec![
                media
                    .clone()
                    .into(),
            ]),
        };
        item.path = item
            .media_sources
            .as_ref()
            .and_then(|s| s.first())
            .and_then(|s| {
                s.path
                    .clone()
            });
        if media.kind != db::MediaKind::Track {
            item.video_type = Some(VideoType::VideoFile);
        }
    }

    if media.kind == db::MediaKind::TvProgram {
        item.channel_id = media
            .parent_id
            .map(|id| id.to_string());
        item.channel_name = media
            .parent
            .as_ref()
            .map(|p| {
                p.title
                    .clone()
            });
        item.channel_primary_image_tag = parent_image_tag(
            media
                .parent
                .as_deref(),
            db::ImageKind::Primary,
        );
        item.location_type = LocationType::Remote;
        item.can_delete = Some(false);
        item.can_download = Some(false);
        item.lock_data = Some(false);
    }

    if media.kind == db::MediaKind::TvChannel {
        item.location_type = LocationType::Remote;
        item.can_delete = Some(false);
        item.can_download = Some(false);
        item.lock_data = Some(false);
        item.is_place_holder = Some(false);
        // Channels use direct-play passthrough — no GStreamer probe needed.
        item.media_sources = Some(vec![MediaSourceInfo {
            id: media.id,
            e_tag: media.id,
            name: Some(
                media
                    .title
                    .clone(),
            ),
            path: media
                .stream_info
                .as_ref()
                .and_then(|si| {
                    si.descriptor
                        .as_http_url()
                        .map(str::to_owned)
                }),
            protocol: MediaProtocol::Http,
            is_remote: true,
            is_infinite_stream: true,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: true,
            type_: MediaSourceType::Placeholder,
            video_type: VideoType::VideoFile,
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
        item.collection_kind = media
            .collection_kind
            .as_ref()
            .map(|k| k.to_string());
        if media.promoted {
            item.type_ = MediaType::CollectionFolder;
            item.display_preferences_id = Some(
                item.id
                    .to_string(),
            );
        } else {
            item.type_ = MediaType::BoxSet;
        }
    }

    if media.kind == db::MediaKind::Folder {
        item.collection_type = Some(CollectionType::Boxsets);
        item.type_ = MediaType::CollectionFolder;
        item.display_preferences_id = Some(
            item.id
                .to_string(),
        );
    }

    item
}
