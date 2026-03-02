pub use shared::sdks::jellyfin::models::*;

use crate::db;
use crate::utils;
use anyhow::Result;

pub trait MediaSourceInfoExt {
    //fn probe_in_place(&mut self) -> anyhow::Result<()>;
    fn probe(&self) -> Result<MediaSourceInfo>;
  }

impl MediaSourceInfoExt for db::Media {

//impl MediaSourceInfoExt for MediaSourceInfo {
    fn probe(&self) -> Result<MediaSourceInfo> {
        let url = self
            .url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing url"))?;
        let mut probed = crate::transcode::probing::probe_media(&url)?;

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
        scroll_direction: data.scroll_direction.parse().ok(),
        show_backdrop: data.show_backdrop,
        remember_sorting: data.remember_sorting,
        sort_order: data.sort_order.parse().ok(),
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
        _ => MediaType::Unknown,
    }
}

pub fn db_media_kind_to_collection_type(kind: db::MediaKind) -> CollectionType {
    match kind {
        db::MediaKind::Movie => CollectionType::Movies,
        db::MediaKind::Series => CollectionType::Tvshows,
        _ => CollectionType::Unknown,
    }
}

pub fn db_user_to_dto(user: db::User) -> UserDto {
    let config = user.configuration.map(|c| c.0).unwrap_or_default();
    let mut policy = user.policy.map(|p| p.0).unwrap_or_default();
    policy.is_administrator = user.is_admin;
    let defaults = UserPolicy::default();
    macro_rules! default_if_empty {
        ($field:ident) => {
            if policy.$field.as_deref() == Some("") {
                policy.$field = defaults.$field;
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

pub fn db_state_to_dto(state: db::UserMediaState) -> UserItemDataDto {
    UserItemDataDto {
        played: Some(state.played_at.is_some()),
        last_played_date: state.played_at.map(|x| x.and_utc()),
        playback_position_ticks: Some(state.playback_position * 10_000),
        play_count: Some(state.play_count as i32),
        is_favorite: Some(state.favorite),
        key: Some(state.media_key),
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
        _ => MediaType::Unknown,
    };

    let mut item = BaseItemDto {
        id: media.id.clone(),
        etag: Some(media.id),
        server_id: utils::server_id(),
        name: Some(media.title.clone()),
        original_title: Some(media.title.clone()),
        overview: media.description.clone(),
        type_,
        parent_id: media.parent_id.clone(),
        remote_trailers: media.trailers.clone().map(|j| j.0),
        series_id: media.parent_id,
        season_id: media.parent_id,
        user_data: media.user_state.clone().map(db_state_to_dto),
        media_type: match media.kind {
            db::MediaKind::Movie | db::MediaKind::Episode => "Video".to_string(),
            _ => "Unknown".to_string(),
        },
        is_place_holder: media.sources.as_ref().map(|sources| sources.is_empty()),
        premiere_date: media.released_at.clone().map(|d| d.and_utc()),
        community_rating: media.rating_audience.clone(),
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
        ),
        backdrop_image_tags: media.backdrop.clone().map(|url| vec![url]),
        provider_ids: Some(ProviderIds {
            imdb: media.imdb_id.clone(),
            aio: media.aio_id.clone(),
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
                        db::RelationRole::Actor => Some("Actor".to_string()),
                        db::RelationRole::Director => Some("Director".to_string()),
                        db::RelationRole::Writer => Some("Writer".to_string()),
                        db::RelationRole::Catalog => None,
                    }),
                    type_: rel.role.as_ref().and_then(|r| match r {
                        db::RelationRole::Actor => Some("Actor".to_string()),
                        db::RelationRole::Director => Some("Director".to_string()),
                        db::RelationRole::Writer => Some("Writer".to_string()),
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
        tags: if media.tags.is_empty() {
            None
        } else {
            Some(media.tags.clone())
        },
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
            collection_catalog_filter: Some(media.catalog_filter_ids()),
            promoted: Some(media.promoted != 0),
        }),
        ..Default::default()
    };

    if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
        item.media_sources = match media.sources.clone() {
            Some(sources) if sources.is_empty() => {
                Some(vec![media.clone().into()])
            }
            Some(sources) => {
                Some(sources.into_iter().map(MediaSourceInfo::from).collect())
            }
            None => None,
        };
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
        item.collection_catalog_filter = if media.collection_catalog_filter.is_some() {
            let ids = media.catalog_filter_ids();
            if ids.is_empty() {
                None
            } else {
                Some(ids.iter().map(|u| u.to_string()).collect())
            }
        } else {
            None
        };
        if media.promoted == 1 {
            item.type_ = MediaType::CollectionFolder;
        } else {
            item.type_ = MediaType::BoxSet;
        }
    }

    if media.kind == db::MediaKind::Folder {
        item.collection_type = Some(CollectionType::Boxsets);
        item.type_ = MediaType::CollectionFolder;
    }

    item
}
