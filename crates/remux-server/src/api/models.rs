pub use remux_sdks::remux::*;

use crate::{common, db};
use anyhow::Result;
use chrono::Datelike;
use uuid::Uuid;

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
        let (mut probed, _) = crate::playback::probe::probe_media(url)?;

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
        custom_name: device
            .custom_name
            .clone(),
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
            db::MediaKind::MusicGenre => MediaType::MusicGenre,
            db::MediaKind::Person => MediaType::Person,
            db::MediaKind::Studio => MediaType::Studio,
            db::MediaKind::Country => MediaType::Studio,
            db::MediaKind::TvChannel => MediaType::TvChannel,
            db::MediaKind::TvProgram => MediaType::Program,
            db::MediaKind::Track => MediaType::Audio,
            db::MediaKind::Album => MediaType::MusicAlbum,
            db::MediaKind::Artist => MediaType::MusicArtist,
            db::MediaKind::Playlist => MediaType::Playlist,
            db::MediaKind::Stream | db::MediaKind::StreamGroup => MediaType::Video,
            db::MediaKind::Subtitle => MediaType::Video,
            db::MediaKind::Intro => MediaType::Video,
        }
    }
}

pub fn db_media_kind_to_collection_type(
    kind: db::CollectionMediaKind,
) -> Option<CollectionType> {
    match kind {
        db::CollectionMediaKind::Movie => Some(CollectionType::Movies),
        db::CollectionMediaKind::Series => Some(CollectionType::Tvshows),
        db::CollectionMediaKind::Mixed => Some(CollectionType::Mixed),
        db::CollectionMediaKind::Music => Some(CollectionType::Music),
        db::CollectionMediaKind::Collection => Some(CollectionType::Boxsets),
        db::CollectionMediaKind::Playlist => Some(CollectionType::Playlists),
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

/// Stable client-facing artwork revision. The database UUID remains the
/// server-side source key; deriving this value lets a compatibility revision
/// invalidate stale client image caches without rewriting media rows.
pub(crate) fn image_tag_for_id(id: Uuid) -> String {
    Uuid::new_v5(&id, b"remux-artwork-v2").to_string()
}

fn media_image_tag(media: &db::Media, kind: db::ImageKind) -> Option<String> {
    media
        .images
        .get(kind)
        .map(|i| image_tag_for_id(i.id))
}

fn parent_image_tag(parent: Option<&db::Media>, kind: db::ImageKind) -> Option<String> {
    parent?
        .images
        .get(kind)
        .map(|i| image_tag_for_id(i.id))
}

/// Jellyfin audio items inherit their album cover when they do not own a
/// primary image. Keep the item id paired with the tag: clients use this pair
/// to construct `/Items/{ParentPrimaryImageItemId}/Images/Primary` URLs.
fn inherited_track_primary_image(media: &db::Media) -> Option<(Uuid, String)> {
    (media.kind == db::MediaKind::Track)
        .then(|| {
            [
                media
                    .parent
                    .as_deref(),
                media
                    .grandparent
                    .as_deref(),
            ]
            .into_iter()
            .flatten()
            .find_map(|parent| {
                parent_image_tag(Some(parent), db::ImageKind::Primary)
                    .map(|tag| (parent.id, tag))
            })
        })
        .flatten()
}

/// The album-art URL fields must always identify the item that owns the tag.
/// A track may carry embedded art without a linked album, in which case the
/// track itself is the image item.  Several music clients use this field
/// instead of `ImageTags.Primary` when rendering the now-playing artwork.
fn track_album_primary_image(media: &db::Media) -> Option<(Uuid, String)> {
    inherited_track_primary_image(media).or_else(|| {
        (media.kind == db::MediaKind::Track).then(|| {
            media_image_tag(media, db::ImageKind::Primary).map(|tag| (media.id, tag))
        })?
    })
}

fn jellyfin_datetime(date: chrono::NaiveDateTime) -> String {
    let utc = date.and_utc();
    let fractional_ticks = utc.timestamp_subsec_nanos() / 100;
    format!(
        "{}.{:07}Z",
        utc.format("%Y-%m-%dT%H:%M:%S"),
        fractional_ticks
    )
}

fn valid_release_date(date: Option<chrono::NaiveDateTime>) -> Option<String> {
    date.filter(|d| d.year() > 1)
        .map(jellyfin_datetime)
}

fn is_music_metadata_item(kind: &db::MediaKind) -> bool {
    matches!(
        kind,
        db::MediaKind::Track
            | db::MediaKind::Album
            | db::MediaKind::Artist
            | db::MediaKind::MusicGenre
            | db::MediaKind::Playlist
    )
}

pub fn db_state_to_dto(
    state: db::UserMediaState,
    media: &db::Media,
) -> UserItemDataDto {
    use crate::common::ToRunTimeTicks;
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
        playback_position_ticks: state
            .playback_position
            .to_ticks(common::TickUnit::Seconds)
            .unwrap_or(0),
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

/// Whether an Audio item should advertise `HasLyrics`. Streaming tracks keep
/// `true` because a lyrics addon can resolve them on demand; a local track only
/// claims lyrics when it actually carries a lyric stream (embedded, or a sidecar
/// injected at probe time). This matches Jellyfin, which reports `HasLyrics:false`
/// for a bare audio file with no lyrics.
fn track_has_lyrics(media: &db::Media) -> bool {
    if media.is_remote_url() {
        return true;
    }
    media
        .probe_data
        .as_ref()
        .is_some_and(|p| {
            p.media_streams
                .iter()
                .any(|s| s.type_ == Some(MediaStreamType::Lyric))
        })
}

pub fn db_media_to_item(media: db::Media, hide_sources: bool) -> BaseItemDto {
    use crate::common::{IntoVec, ToRunTimeTicks};

    let type_ = media
        .kind
        .clone()
        .into();

    let mut item = BaseItemDto {
        id: media
            .id
            .clone(),
        // Discrete caches item DTOs by Etag. Keep the item id namespace but
        // revision artwork-bearing DTOs so clients refresh image references
        // after a server-side artwork repair.
        etag: Some(Uuid::new_v5(&media.id, b"remux-artwork-v3")),
        server_id: common::server_id(),
        name: Some(
            media
                .title
                .clone(),
        ),
        overview: media
            .description
            .clone(),
        play_access: Some("Full".to_string()),
        can_delete: Some(false),
        can_download: Some(false),
        has_lyrics: (media.kind == db::MediaKind::Track)
            .then(|| track_has_lyrics(&media)),
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
            | db::MediaKind::TvProgram
            | db::MediaKind::Intro => MediaType::Video,
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
        is_series: matches!(media.program_kind, Some(db::ProgramKind::Series))
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
        premiere_date: valid_release_date(media.released_at),
        production_year: media
            .released_at
            .filter(|d| d.year() > 1)
            .map(|d| d.year() as i64),
        community_rating: media
            .rating_audience
            .map(|r| (r * 10.0).round() / 10.0),
        critic_rating: media
            .rating_critic
            .clone(),
        official_rating: media
            .certification
            .clone(),
        parent_index_number: media.parent_idx,
        image_tags: Some(ImageTags {
            primary: media_image_tag(&media, db::ImageKind::Primary)
                .or_else(|| inherited_track_primary_image(&media).map(|(_, tag)| tag))
                .or_else(|| {
                    // For collections/folders with no poster image, set a synthetic
                    // tag so clients know to request the generated placeholder.
                    matches!(
                        media.kind,
                        db::MediaKind::Collection | db::MediaKind::Folder
                    )
                    .then(|| {
                        media
                            .id
                            .to_string()
                    })
                }),
            logo: media_image_tag(&media, db::ImageKind::Logo),
            backdrop: media_image_tag(&media, db::ImageKind::Backdrop),
            thumb: media_image_tag(&media, db::ImageKind::Thumb),
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
                .as_deref()
                .or_else(|| {
                    if matches!(
                        media.kind,
                        db::MediaKind::Episode | db::MediaKind::Season
                    ) {
                        media
                            .external_ids
                            .series_imdb
                            .as_deref()
                    } else {
                        None
                    }
                })
                .map(|s| s.to_string()),
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
                        secs.to_ticks(common::TickUnit::Seconds)
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
                    .filter(|(_, m)| {
                        matches!(
                            m.kind,
                            db::MediaKind::Genre | db::MediaKind::MusicGenre
                        )
                    })
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
                    .filter(|(_, m)| {
                        matches!(
                            m.kind,
                            db::MediaKind::Genre | db::MediaKind::MusicGenre
                        )
                    })
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
                                db::RelationRole::Actor => Some(
                                    if media.kind == db::MediaKind::Episode {
                                        "GuestStar"
                                    } else {
                                        "Actor"
                                    }
                                    .to_string(),
                                ),
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
        album_primary_image_tag: track_album_primary_image(&media).map(|(_, tag)| tag),
        album_primary_image_item_id: track_album_primary_image(&media)
            .map(|(id, _)| id.to_string()),
        parent_primary_image_item_id: inherited_track_primary_image(&media)
            .map(|(id, _)| id.to_string()),
        parent_primary_image_tag: inherited_track_primary_image(&media)
            .map(|(_, tag)| tag),
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
        // Jellyfin initializes these arrays for music DTOs, including sparse
        // tracks with no linked artist. Some clients distinguish [] from an
        // omitted field, so preserve the stock wire shape here.
        album_artists: is_music_metadata_item(&media.kind).then(|| {
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
                .unwrap_or_default()
        }),
        artists: is_music_metadata_item(&media.kind).then(|| {
            media
                .grandparent
                .as_ref()
                .map(|gp| {
                    vec![
                        gp.title
                            .clone(),
                    ]
                })
                .unwrap_or_default()
        }),
        artist_items: is_music_metadata_item(&media.kind).then(|| {
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
                .unwrap_or_default()
        }),
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
            media_image_tag(&media, db::ImageKind::Primary)
                .and_then(|_| {
                    media
                        .images
                        .get(db::ImageKind::Primary)
                })
                .or_else(|| {
                    inherited_track_primary_image(&media).and_then(|(parent_id, _)| {
                        [
                            media
                                .parent
                                .as_deref(),
                            media
                                .grandparent
                                .as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .find(|parent| parent.id == parent_id)
                        .and_then(|parent| {
                            parent
                                .images
                                .get(db::ImageKind::Primary)
                        })
                    })
                })
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
                    // Music cover art is conventionally square. Reporting the
                    // poster fallback (0.6) when an upstream image has no
                    // stored dimensions makes music clients lay it out as a
                    // poster and can suppress the artwork request entirely.
                    db::MediaKind::Track
                    | db::MediaKind::Album
                    | db::MediaKind::Artist => 1.0,
                    _ => 0.6,
                }),
        ),
        remux: Some(RemuxInfo {
            recommendation_explanation: None,
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
            digital_release_date: valid_release_date(media.digital_released_at),
            latest_auto_unplayed: media.collection_latest_auto_unplayed,
            latest_sort_digital: media.collection_latest_sort_digital,
            collection_source: media
                .collection_source
                .clone(),
            collection_default_sort: media
                .collection_default_sort
                .clone(),
            collection_default_sort_order: media
                .collection_default_sort_order
                .clone(),
        }),
        enable_media_source_display: Some(true),
        date_created: Some(jellyfin_datetime(media.created_at)),
        original_language: media
            .original_language
            .clone(),
        production_locations: {
            let from_relations: Vec<String> = media
                .relations
                .as_ref()
                .map(|rels| {
                    rels.iter()
                        .filter(|(_, m)| m.kind == db::MediaKind::Country)
                        .map(|(_, m)| {
                            m.title
                                .clone()
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !from_relations.is_empty() {
                Some(from_relations)
            } else {
                (media.kind == db::MediaKind::Person)
                    .then(|| {
                        media
                            .country
                            .clone()
                            .map(|c| vec![c])
                    })
                    .flatten()
            }
        },
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
    if !hide_sources
        && (media.kind == db::MediaKind::Movie
            || media.kind == db::MediaKind::Episode
            || media.kind == db::MediaKind::Track)
    {
        item.can_download = Some(true);
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
                    .as_deref()
            })
            .map(|p| format!("{}.strm", p));
        // Jellyfin sets item-level Container from the primary media source.
        item.container = item
            .media_sources
            .as_ref()
            .and_then(|s| s.first())
            .and_then(|s| {
                s.container
                    .clone()
            });
        // Jellyfin mirrors the primary source's streams at the item level.
        if let Some(first) = item
            .media_sources
            .as_ref()
            .and_then(|s| s.first())
        {
            if !first
                .media_streams
                .is_empty()
            {
                item.media_streams = Some(
                    first
                        .media_streams
                        .clone(),
                );
            }
        }
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
            video_type: Some(VideoType::VideoFile),
            ..Default::default()
        }]);
    }

    if media.kind == db::MediaKind::Collection {
        item.collection_type = media
            .collection_media_kind
            .clone()
            .and_then(db_media_kind_to_collection_type);
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

// ── Remote Search DTOs ──────────────────────────────────────────────────────

#[remux_macros::query]
#[derive(Debug, Clone, Default)]
pub struct ItemLookupInfo {
    pub name: Option<String>,
    pub year: Option<i64>,
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
}

#[remux_macros::query]
#[derive(Debug, Clone, Default)]
pub struct RemoteSearchQuery {
    pub search_info: Option<ItemLookupInfo>,
    pub item_id: Option<uuid::Uuid>,
    pub search_provider_name: Option<String>,
    pub include_disabled_providers: Option<bool>,
}

#[remux_macros::query]
#[derive(Debug, Clone, Default)]
pub struct ApplySearchResultRequest {
    pub name: Option<String>,
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
    pub production_year: Option<i64>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteSearchResult {
    pub name: Option<String>,
    pub production_year: Option<i64>,
    pub image_url: Option<String>,
    pub search_provider_name: Option<String>,
    pub provider_ids: std::collections::HashMap<String, String>,
    pub overview: Option<String>,
    pub premiere_date: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteSubtitleInfo {
    pub id: String,
    pub name: Option<String>,
    pub provider_name: Option<String>,
    pub three_letter_iso_language_name: Option<String>,
    pub format: Option<String>,
    pub is_hash_match: Option<bool>,
    pub ai_translated: Option<bool>,
    pub machine_translated: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_track_keeps_initialized_music_arrays() {
        let item = db_media_to_item(
            db::Media {
                title: "Sparse Track".to_string(),
                kind: db::MediaKind::Track,
                ..Default::default()
            },
            true,
        );
        let json = serde_json::to_value(item).unwrap();

        assert_eq!(json.get("Artists"), Some(&serde_json::json!([])));
        assert_eq!(json.get("ArtistItems"), Some(&serde_json::json!([])));
        assert_eq!(json.get("AlbumArtists"), Some(&serde_json::json!([])));
    }
}
