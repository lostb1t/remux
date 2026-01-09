use crate::utils::get_uuid;

use super::DbConn;
use super::schema::{media, provider_ids};
use chrono::{DateTime, Utc};
use diesel::{expression::AsExpression, prelude::*, sql_types::Text};
use diesel_derive_enum::DbEnum;
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};
use thiserror::Error;
use uuid::Uuid;


#[derive(
    Default,
    EnumString,
    Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    DbEnum,
    Eq,
    FromSqlRow,
)]
#[serde(rename_all = "lowercase")]
#[DieselType = "EventTypeMapping"]
pub enum MediaKind {
    Movie,
    Series,
    Season,
    Episode,
    Catalog,
    Source,
    #[default]
    Unknown,
}

#[derive(
    EnumString,
    Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    DbEnum,
    // DieselNewType,
    // AsExpression,
    // FromSqlRow,
)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Imdb,
    Aio,
}

#[derive(
    Debug,
    Clone,
    default2::Default,
    Serialize,
    Deserialize,
    Queryable,
    Identifiable,
    Insertable,
)]
#[diesel(table_name = media)]
pub struct Media {
    #[default(get_uuid())]
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
    pub parent_id: Option<String>,
    pub idx: Option<i64>,
    pub released_at: Option<DateTime<Utc>>,
    pub runtime: Option<i64>, // Opslaan als seconden
    pub rating_critic: Option<i64>,
    pub rating_audience: Option<i64>,
    pub poster: Option<String>,
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub remote_data: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Insertable)]
#[diesel(table_name = provider_ids)]
pub struct ProviderIds {
    pub media_id: String,
    pub kind: Provider,
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaFilter {
    pub id: Option<String>,
    pub kind: Option<Vec<MediaKind>>,
    pub parent_id: Option<String>,
    pub idx: Option<i64>,
}

#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Invalid media: {0}")]
    ValidationError(String),
    #[error("Database error: {0}")]
    DatabaseError(#[from] diesel::result::Error),
}

impl Media {
    pub fn validate(&self) -> Result<(), MediaError> {
        match self.kind {
            MediaKind::Season | MediaKind::Episode if self.idx.is_none() => {
                Err(MediaError::ValidationError(format!(
                    "{:?} requires an index number",
                    self.kind
                )))
            }
            _ => Ok(()),
        }
    }

    pub fn save(&mut self, pool: &DbConn) -> Result<(), MediaError> {
        self.validate()?;
        let updated_at = Utc::now();
        let mut conn = pool.get_conn()?;
        diesel::insert_into(media::table)
            .values(&*self)
            .on_conflict(media::id)
            .do_update()
            .set(&*self)
            .execute(&mut conn)?;

        Ok(())
    }

    pub fn get_by_id(pool: &DbConn, id: &str) -> Result<Option<Self>> {
        let mut conn = pool.get_conn()?;
        Ok(media::table
            .filter(media::id.eq(id))
            .first(&mut conn)
            .optional()
            .map_err(|e| e.into())?)
    }

    pub fn get_with_filter(pool: &DbConn, filter: &MediaFilter) -> Result<Vec<Self>> {
        let mut query = media::table.into_boxed();
        let mut conn = pool.get_conn()?;
        if let Some(ref parent_id) = filter.parent_id {
            query = query.filter(media::parent_id.eq(parent_id));
        }

        if let Some(ref kinds) = filter.kind {
            query = query.filter(media::kind.eq_any(kinds));
        }

        if let Some(idx) = filter.idx {
            query = query.filter(media::idx.eq(idx));
        }

        query.load(conn).map_err(|e| e.into())
    }

    pub fn into_base_item(self, pool: &DbConn) -> Result<jellyfin::BaseItemDto> {
        let mut conn = pool.get_conn()?;
        let provider_ids = ProviderIds::get_by_media_id(conn, &self.id)?;

        let mut item = jellyfin::BaseItemDto {
            id: self.id,
            server_id: server_id(),
            type_: self.kind.into(),
            parent_id: self.parent_id,
            index_number: self.idx,
            name: Some(match self.kind {
                MediaKind::Episode => format!("Episode {}", self.idx.unwrap_or(0)),
                MediaKind::Season => format!("Season {}", self.idx.unwrap_or(0)),
                _ => self.title.clone(),
            }),
            is_folder: matches!(self.kind, MediaKind::Series | MediaKind::Season),
            ..Default::default()
        };

        Ok(item)
    }

    pub fn parent(&self, pool: &DbConn) -> Result<Option<Self>> {
        if let Some(parent_id) = &self.parent_id {
            Self::get_by_id(pool, parent_id)
        } else {
            Ok(None)
        }
    }
}

// impl ProviderIds {
//     pub fn save(&self, pool: &DbConn) -> Result<usize> {
//         let mut conn = pool.get_conn()?;
//         Ok(diesel::insert_into(provider_ids::table)
//             .values(self)
//             .on_conflict((provider_ids::media_id, provider_ids::kind))
//             .do_update()
//             .set(provider_ids::id.eq(&self.id))
//             .execute(&mut conn)?)
//     }

//     pub fn delete(&self, pool: &DbConn) -> Result<()> {
//         let mut conn = pool.get_conn()?;
//         Ok(diesel::delete(
//             provider_ids::table
//                 .filter(provider_ids::media_id.eq(&self.media_id))
//                 .filter(provider_ids::kind.eq(&self.kind)),
//         )
//         .execute(&mut conn)?)
//     }

//     pub fn get_by_media_id(conn: &DbConn, media_id: &str) -> Result<Option<Self>> {
//         let mut pool = conn.get_conn()?;
//         provider_ids::table
//             .filter(provider_ids::media_id.eq(media_id))
//             .first(pool)
//             .optional()
//     }

//     pub fn get_by_id(conn: &DbConn, kind: Provider, id: &str) -> Result<Option<Self>> {
//         let mut pool = conn.get_conn()?;
//         provider_ids::table
//             .filter(provider_ids::kind.eq(kind))
//             .filter(provider_ids::id.eq(id))
//             .first(pool)
//             .optional()
//     }
// }

impl From<MediaKind> for jellyfin::MediaType {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Movie => jellyfin::MediaType::Movie,
            MediaKind::Series => jellyfin::MediaType::Series,
            MediaKind::Season => jellyfin::MediaType::Season,
            MediaKind::Episode => jellyfin::MediaType::Episode,
            MediaKind::Catalog => jellyfin::MediaType::BoxSet,
            _ => jellyfin::MediaType::Unknown,
        }
    }
}

impl From<sdks::aio::Meta> for Vec<Media> {
    fn from(meta: sdks::aio::Meta) -> Self {
        let mut media_instances = Vec::new();
        let media_kind = MediaKind::from(meta.media_type.clone());

        let media = Media {
            id: get_uuid(),
            title: meta.name.unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta.released,
            runtime: meta.runtime.map(|d| d.num_seconds()),
            rating_audience: meta.imdb_rating,
            poster: meta.poster,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Default::default()
        };
        media_instances.push(media);

        if let MediaKind::Series = media_kind {
            if let Some(episodes) = meta.videos {
                let seasons: std::collections::BTreeMap<i64, Vec<sdks::aio::Episode>> =
                    episodes
                        .into_iter()
                        .filter_map(|ep| ep.season.map(|s| (s, ep)))
                        .fold(
                            std::collections::BTreeMap::new(),
                            |mut acc, (season, ep)| {
                                acc.entry(season).or_default().push(ep);
                                acc
                            },
                        );

                for (season_idx, episodes) in seasons {
                    let season_media = Media {
                        id: get_uuid(),
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        ..Default::default()
                    };
                    media_instances.push(season_media);

                    for episode in episodes {
                        let episode_media = Media {
                            id: get_uuid(),
                            kind: MediaKind::Episode,
                            title: episode.name.unwrap_or_default(),
                            idx: episode.episode,
                            released_at: episode.released,
                            runtime: episode.runtime.map(|d| d.num_seconds()),
                            created_at: Utc::now(),
                            updated_at: Utc::now(),
                            ..Default::default()
                        };
                        media_instances.push(episode_media);
                    }
                }
            }
        }

        media_instances
    }
}
