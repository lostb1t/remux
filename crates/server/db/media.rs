use super::{FilterResult, QueryBuilderExt};
use crate::aio;
use crate::jellyfin;
use crate::sdks;
use crate::utils::IntoVec;
use crate::utils::get_uuid;
use crate::utils::server_id;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use axum::ServiceExt;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::middleware;
use axum::middleware::Next;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::{
    Json, Router,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
};
use axum_anyhow::ApiError;
use axum_anyhow::on_error;
use axum_anyhow::set_expose_errors;
use axum_anyhow::{ApiResult, OptionExt, ResultExt};
use chrono::prelude::*;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use config;
use config::Config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use reqwest;
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use sqlx::SqlitePool;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use timed;
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing;
use tracing::debug;
use tracing::info;
use tracing::instrument;
use tracing::trace;
use tracing::warn;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use url::Url;
use uuid::{Uuid, uuid};

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
//#[sqlx(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Series,
    Season,
    Episode,
    Person,
    Studio,
    Genre,
    Collection,
    // AIO import source catalog item
    Catalog,
    // purely here for jf
    Folder,
    Source,
    #[default]
    Unknown,
}

impl From<String> for MediaKind {
    fn from(s: String) -> Self {
        Self::try_from(s.as_str()).unwrap_or(MediaKind::Unknown)
    }
}

impl From<sdks::aio::MediaType> for MediaKind {
    fn from(media_type: sdks::aio::MediaType) -> Self {
        match media_type {
            sdks::aio::MediaType::Movie => MediaKind::Movie,
            sdks::aio::MediaType::Series | sdks::aio::MediaType::Tv => {
                MediaKind::Series
            }
            _ => todo!(),
        }
    }
}

pub fn media_kind_to_aio(kind: &MediaKind) -> sdks::aio::MediaType {
    match kind {
        MediaKind::Movie => sdks::aio::MediaType::Movie,
        MediaKind::Series | MediaKind::Season | MediaKind::Episode => {
            sdks::aio::MediaType::Series
        }
        _ => sdks::aio::MediaType::Movie,
    }
}

impl From<jellyfin::MediaType> for MediaKind {
    fn from(media_type: jellyfin::MediaType) -> Self {
        match media_type {
            jellyfin::MediaType::Movie => MediaKind::Movie,
            jellyfin::MediaType::Series => MediaKind::Series,
            jellyfin::MediaType::Season => MediaKind::Season,
            jellyfin::MediaType::Episode => MediaKind::Episode,
            jellyfin::MediaType::BoxSet => MediaKind::Collection,
            _ => MediaKind::Unknown,
        }
    }
}

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum CollectionKind {
    #[default]
    Manual,
    Smart,
}

impl TryFrom<String> for CollectionKind {
    type Error = strum::ParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum RelationRole {
    #[default]
    Actor,
    Director,
    Writer,
    Catalog,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaRelation {
    #[default(get_uuid())]
    pub relation_id: Uuid,
    pub left_media_id: Uuid,
    pub right_media_id: Uuid,
    pub weight: Option<i64>,
    pub role: Option<RelationRole>,
}

impl MediaRelation {
    pub async fn upsert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = db.begin().await?;
        const BATCH_SIZE: usize = 900;

        for chunk in items.chunks(BATCH_SIZE) {
            let mut qb = sqlx::QueryBuilder::new(
                "INSERT INTO media_relations (relation_id, left_media_id, right_media_id, weight, role) ",
            );

            qb.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.relation_id)
                    .push_bind(&item.left_media_id)
                    .push_bind(&item.right_media_id)
                    .push_bind(&item.weight)
                    .push_bind(&item.role);
            });

            qb.push(" ON CONFLICT (left_media_id, right_media_id, COALESCE(role, '')) DO UPDATE SET weight = excluded.weight");

            qb.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn get_by_media_id(
        db: &SqlitePool,
        media_id: &Uuid,
    ) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Self>(
            "SELECT * FROM media_relations WHERE left_media_id = $1 ORDER BY weight ASC",
        )
        .bind(media_id)
        .fetch_all(db)
        .await?;

        Ok(rows)
    }
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaFilter {
    pub id: Option<Vec<Uuid>>,
    pub kind: Option<Vec<MediaKind>>,
    pub parent_id: Option<Uuid>,
    pub imdb_id: Option<String>,
    pub aio_id: Option<String>,
    pub promoted: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub recursive: bool,
    pub total_count: bool,
    pub include_user_state: bool,
    pub user_state: Option<super::UserMediaStateFilter>,
    pub genre_ids: Option<Vec<Uuid>>,
    pub catalog_ids: Option<Vec<Uuid>>,
    pub studio_ids: Option<Vec<Uuid>>,
    pub person_ids: Option<Vec<Uuid>>,
    pub years: Option<Vec<i64>>,
    pub official_ratings: Option<Vec<String>>,
    pub name_starts_with: Option<String>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_less_than: Option<String>,
    pub title_contains: Option<String>,
    pub index_number: Option<i64>,
    pub has_trailer: Option<bool>,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    // shared
    //#[sqlx(try_from="String")]
    #[default(get_uuid())]
    pub id: Uuid,
    pub title: String,
    pub kind: MediaKind,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub refreshed_at: Option<NaiveDateTime>,

    // meta
    pub description: Option<String>,
    pub released_at: Option<NaiveDateTime>,
    pub trailers: Option<sqlx::types::Json<Vec<String>>>,
    // in seconds
    pub runtime: Option<i64>,
    pub rating_critic: Option<f64>,
    pub rating_audience: Option<f64>,
    pub certification: Option<String>,
    pub poster: Option<String>,
    pub logo: Option<String>,
    pub backdrop: Option<String>,
    pub idx: Option<i64>,
    pub parent_idx: Option<i64>,
    pub parent_id: Option<Uuid>,
    pub series_imdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub aio_id: Option<String>,
    //pub media_key: Option<String>,
    //pub series_id: Option<Uuid>,
    //pub season_id: Option<Uuid>,
    //pub description: Option<String>,
    #[sqlx(skip)]
    pub sources: Option<Vec<Media>>,
    #[sqlx(skip)]
    pub seasons: Option<Vec<Media>>,
    #[sqlx(skip)]
    pub episodes: Option<Vec<Media>>,
    #[sqlx(skip)]
    pub user_state: Option<super::UserMediaState>,
    #[sqlx(skip)]
    pub relations: Option<Vec<(MediaRelation, Media)>>,

    // stream
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub remote_data: Option<String>,

    // collection
    pub promoted: i64,
    // CollectionKind
    pub collection_kind: Option<CollectionKind>,
    // MediaKind
    pub collection_media_kind: Option<MediaKind>,
    pub collection_max_items: Option<i64>,
    // JSON array of catalog media item UUIDs (for smart collections with catalog filter)
    pub collection_catalog_filter: Option<String>,
}

impl Media {
    /// Parse the JSON catalog filter into a list of UUIDs.
    pub fn catalog_filter_ids(&self) -> Vec<Uuid> {
        self.collection_catalog_filter
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect()
    }
}

// #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
// pub struct SqlBool(pub bool);

// impl From<i32> for SqlBool {
//     fn from(value: i32) -> Self {
//         match value {
//             0 => Self(false),
//             1 => Self(true),
//             _ => panic!("invalid boolean value {value}"),
//         }
//     }
// }

#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Invalid media: {0}")]
    ValidationError(String),
}

impl Media {
    pub fn is_promoted(&self) -> bool {
        match self.promoted {
            0 => false,
            1 => true,
            _ => panic!("invalid boolean value"),
        }
    }

    pub fn validate(&self) -> Result<(), MediaError> {
        match self.kind {
            MediaKind::Season | MediaKind::Episode if self.idx.is_none() => {
                Err(MediaError::ValidationError(format!(
                    "{:?} requires an index number",
                    self.kind
                )))
            }
            _ => Ok(()),
        }?;

        if self.kind == MediaKind::Movie || self.kind == MediaKind::Series {
            if self.imdb_id.is_none() {
                return Err(MediaError::ValidationError(format!(
                    "{:?} requires an imdb id",
                    self.kind
                )));
            }
        }

        if self.kind == MediaKind::Unknown {
            return Err(MediaError::ValidationError(format!(
                "{:?} requires an kind",
                self.kind
            )));
        }

        Ok(())
    }

    pub async fn save(&mut self, db: &sqlx::SqlitePool) -> Result<()> {
        self.validate()?;
        let updated_at = Utc::now().naive_utc();

        sqlx::query(
        r#"
        INSERT INTO media (
            id, title, kind, parent_id, idx, released_at, runtime,
            rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, collection_kind, collection_media_kind, collection_max_items, collection_catalog_filter,
            remote_data, series_imdb_id, aio_id, imdb_id, created_at, updated_at, certification, parent_idx
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29)
        ON CONFLICT (id) DO UPDATE SET
            title = excluded.title,
            kind = excluded.kind,
            idx = excluded.idx,
            released_at = excluded.released_at,
            runtime = excluded.runtime,
            rating_critic = excluded.rating_critic,
            rating_audience = excluded.rating_audience,
            poster = excluded.poster,
            logo = excluded.logo,
            backdrop = excluded.backdrop,
            description = excluded.description,
            trailers = excluded.trailers,
            url = excluded.url,
            probe_data = excluded.probe_data,
            remote_data = excluded.remote_data,
            series_imdb_id = excluded.series_imdb_id,
            imdb_id = excluded.imdb_id,
            aio_id = excluded.aio_id,
            promoted = excluded.promoted,
            collection_kind = excluded.collection_kind,
            collection_media_kind = excluded.collection_media_kind,
            collection_max_items = excluded.collection_max_items,
            collection_catalog_filter = excluded.collection_catalog_filter,
            updated_at = excluded.updated_at,
            certification = excluded.certification,
            parent_idx = excluded.parent_idx
        "#,
        )
        .bind(self.id)
        .bind(&self.title)
        .bind(&self.kind)
        .bind(self.parent_id)
        .bind(self.idx)
        .bind(self.released_at)
        .bind(self.runtime)
        .bind(self.rating_critic)
        .bind(self.rating_audience)
        .bind(&self.poster)
        .bind(&self.logo)
        .bind(&self.backdrop)
        .bind(&self.description)
        .bind(&self.trailers)
        .bind(&self.url)
        .bind(&self.probe_data)
        .bind(self.promoted)
        .bind(&self.collection_kind)
        .bind(&self.collection_media_kind)
        .bind(self.collection_max_items)
        .bind(&self.collection_catalog_filter)
        .bind(&self.remote_data)
        .bind(&self.series_imdb_id)
        .bind(&self.aio_id)
        .bind(&self.imdb_id)
        .bind(self.created_at)
        .bind(updated_at)
        .bind(&self.certification)
        .bind(self.parent_idx)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn insert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = db.begin().await?;
        const BATCH_SIZE: usize = 900;

        for chunk in items.chunks(BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, collection_kind, collection_media_kind,
                remote_data, series_imdb_id, imdb_id, aio_id, created_at, updated_at, certification, parent_idx
            )",
        );
            for item in chunk {
                item.validate()?;
            }
            query_builder.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.id)
                    .push_bind(&item.title)
                    .push_bind(&item.kind)
                    .push_bind(&item.parent_id)
                    .push_bind(&item.idx)
                    .push_bind(&item.released_at)
                    .push_bind(&item.runtime)
                    .push_bind(&item.rating_critic)
                    .push_bind(&item.rating_audience)
                    .push_bind(&item.poster)
                    .push_bind(&item.logo)
                    .push_bind(&item.backdrop)
                    .push_bind(&item.description)
                    .push_bind(&item.trailers)
                    .push_bind(&item.url)
                    .push_bind(&item.probe_data)
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(&item.remote_data)
                    .push_bind(&item.series_imdb_id)
                    .push_bind(&item.imdb_id)
                    .push_bind(&item.aio_id)
                    .push_bind(&item.created_at)
                    .push_bind(Utc::now())
                    .push_bind(&item.certification)
                    .push_bind(&item.parent_idx);
            });

            query_builder.push(" ON CONFLICT DO NOTHING");

            query_builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = db.begin().await?;
        const BATCH_SIZE: usize = 900;

        for chunk in items.chunks(BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, collection_kind, collection_media_kind,
                remote_data, series_imdb_id, imdb_id, aio_id, created_at, updated_at, certification, parent_idx
            )",
        );

            query_builder.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.id)
                    .push_bind(&item.title)
                    .push_bind(&item.kind)
                    .push_bind(&item.parent_id)
                    .push_bind(&item.idx)
                    .push_bind(&item.released_at)
                    .push_bind(&item.runtime)
                    .push_bind(&item.rating_critic)
                    .push_bind(&item.rating_audience)
                    .push_bind(&item.poster)
                    .push_bind(&item.logo)
                    .push_bind(&item.backdrop)
                    .push_bind(&item.description)
                    .push_bind(&item.trailers)
                    .push_bind(&item.url)
                    .push_bind(&item.probe_data)
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(&item.remote_data)
                    .push_bind(&item.series_imdb_id)
                    .push_bind(&item.imdb_id)
                    .push_bind(&item.aio_id)
                    .push_bind(&item.created_at)
                    .push_bind(Utc::now())
                    .push_bind(&item.certification)
                    .push_bind(&item.parent_idx);
            });

            query_builder.push(
                " ON CONFLICT DO UPDATE SET
                title = excluded.title,
                idx = excluded.idx,
                released_at = excluded.released_at,
                runtime = excluded.runtime,
                rating_critic = excluded.rating_critic,
                rating_audience = excluded.rating_audience,
                poster = excluded.poster,
                logo = excluded.logo,
                backdrop = excluded.backdrop,
                description = excluded.description,
                trailers = excluded.trailers,
                url = excluded.url,
                aio_id = excluded.aio_id,
                imdb_id = excluded.imdb_id,
                probe_data = excluded.probe_data,
                remote_data = excluded.remote_data,
                series_imdb_id = excluded.series_imdb_id,
                updated_at = excluded.updated_at,
                promoted = excluded.promoted,
                certification = excluded.certification,
                parent_id = excluded.parent_id,
                parent_idx = excluded.parent_idx",
            );

            query_builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Return distinct Genre records linked (via media_relations) to media of the given kinds.
    /// If `related_kinds` is empty, all genres are returned.
    pub async fn get_genres(
        db: &SqlitePool,
        related_kinds: &[MediaKind],
    ) -> Result<Vec<Self>> {
        let mut qb = sqlx::QueryBuilder::new("SELECT DISTINCT g.* FROM media g");

        if !related_kinds.is_empty() {
            qb.push(" JOIN media_relations mr ON mr.right_media_id = g.id");
            qb.push(" JOIN media m ON mr.left_media_id = m.id");
            qb.push(" WHERE g.kind = 'genre' AND m.kind IN (");
            let mut sep = qb.separated(", ");
            for k in related_kinds {
                sep.push_bind(k);
            }
            qb.push(")");
        } else {
            qb.push(" WHERE g.kind = 'genre'");
        }

        qb.push(" ORDER BY g.title ASC");

        Ok(qb.build_query_as::<Self>().fetch_all(db).await?)
    }

    pub async fn get_by_id(
        db: &SqlitePool,
        id: &Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM media
        WHERE id = $1
        "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_by_filter(
        db: &SqlitePool,
        filter: &MediaFilter,
    ) -> Result<FilterResult<Media>> {
        let use_recursive = filter.recursive && filter.parent_id.is_some();

        let mut count_qb;
        let mut records_qb;

        if use_recursive {
            let parent_id = filter.parent_id.as_ref().unwrap();

            count_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            count_qb.push_bind(parent_id);
            count_qb.push(
                " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                ) SELECT COUNT(*) as count FROM media WHERE id IN (SELECT id FROM subtree) AND 1=1",
            );

            records_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            records_qb.push_bind(parent_id);
            records_qb.push(
                " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                ) SELECT * FROM media WHERE id IN (SELECT id FROM subtree) AND 1=1",
            );
        } else {
            count_qb = sqlx::QueryBuilder::new(
                "SELECT COUNT(*) as count FROM media WHERE 1=1",
            );
            records_qb = sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");
        }

        for qb in [&mut count_qb, &mut records_qb] {
            if !use_recursive {
                if let Some(parent_id) = &filter.parent_id {
                    qb.push(" AND parent_id = ").push_bind(parent_id);
                }
            }
            if let Some(aio_id) = &filter.aio_id {
                qb.push(" AND aio_id = ").push_bind(aio_id);
            }
            if let Some(promoted) = &filter.promoted {
                qb.push(" AND promoted = ").push_bind(promoted);
            }
            if let Some(kind) = &filter.kind {
                qb.push_in("kind", &kind);
            }
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }
            if let Some(imdb_id) = &filter.imdb_id {
                qb.push(" AND imdb_id = ").push_bind(imdb_id);
            }

            if let Some(genre_ids) = &filter.genre_ids {
                if !genre_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in genre_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(catalog_ids) = &filter.catalog_ids {
                if !catalog_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.role = 'catalog' AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in catalog_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(user_state_filter) = &filter.user_state {
                // favorite — always uses EXISTS
                if let Some(favorite) = &user_state_filter.favorite {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_key = media.aio_id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.favorite = ").push_bind(favorite).push(")");
                }

                // played=true — EXISTS with play_count > 0
                if user_state_filter.played == Some(true) {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_key = media.aio_id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.play_count > 0)");
                }

                // played=false (unplayed) — NOT EXISTS with play_count > 0
                if user_state_filter.played == Some(false) {
                    qb.push(" AND NOT EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_key = media.aio_id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.play_count > 0)");
                }

                // resumable — EXISTS with playback_position > 0 AND play_count = 0
                if user_state_filter.resumable == Some(true) {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_key = media.aio_id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.playback_position > 0 AND ums.play_count = 0)");
                }
            }

            if let Some(years) = &filter.years {
                if !years.is_empty() {
                    qb.push(" AND CAST(strftime('%Y', released_at) AS INTEGER) IN (");
                    let mut sep = qb.separated(", ");
                    for y in years {
                        sep.push_bind(y);
                    }
                    qb.push(")");
                }
            }

            if let Some(ratings) = &filter.official_ratings {
                if !ratings.is_empty() {
                    qb.push(" AND certification IN (");
                    let mut sep = qb.separated(", ");
                    for r in ratings {
                        sep.push_bind(r);
                    }
                    qb.push(")");
                }
            }

            if let Some(s) = &filter.name_starts_with {
                // LIKE is case-insensitive for ASCII in SQLite; no UPPER() needed.
                // A COLLATE NOCASE index on title can satisfy this as a prefix scan.
                qb.push(" AND title LIKE ").push_bind(format!("{}%", s));
            }

            if let Some(s) = &filter.name_starts_with_or_greater {
                qb.push(" AND title >= ").push_bind(s.clone()).push(" COLLATE NOCASE");
            }

            if let Some(s) = &filter.name_less_than {
                qb.push(" AND title < ").push_bind(s.clone()).push(" COLLATE NOCASE");
            }

            if let Some(s) = &filter.title_contains {
                qb.push(" AND title LIKE ")
                    .push_bind(format!("%{}%", s));
            }

            if let Some(idx) = &filter.index_number {
                qb.push(" AND idx = ").push_bind(idx);
            }

            if let Some(true) = &filter.has_trailer {
                qb.push(" AND json_array_length(trailers) > 0");
            }
            if let Some(false) = &filter.has_trailer {
                qb.push(
                    " AND (trailers IS NULL OR json_array_length(trailers) = 0)",
                );
            }

            if let Some(studio_ids) = &filter.studio_ids {
                if !studio_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in studio_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(person_ids) = &filter.person_ids {
                if !person_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in person_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }
        }

        if let Some(limit) = &filter.limit {
            records_qb.push(" LIMIT ").push_bind(limit);
        }
        if let Some(offset) = &filter.offset {
            records_qb.push(" OFFSET ").push_bind(offset);
        }

        let (count, records_result) = tokio::join!(
            async {
                let query = count_qb.build();
                let row = query.fetch_one(db).await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<Media>();
                query.fetch_all(db).await
            }
        );

        let mut records = records_result?;

        if filter.include_user_state {
            if let Some(user_state_filter) = &filter.user_state {
                let media_keys: Vec<String> =
                    records.iter().map(|m| m.media_key()).collect();

                let states = super::UserMediaState::get_by_filter(
                    db,
                    &super::UserMediaStateFilter {
                        user_id: user_state_filter.user_id,
                        media_key: Some(media_keys),
                        played: user_state_filter.played,
                        favorite: user_state_filter.favorite,
                        ..Default::default()
                    },
                )
                .await?
                .records;

                let states_map: HashMap<String, super::UserMediaState> = states
                    .into_iter()
                    .map(|state| (state.media_key.clone(), state))
                    .collect();

                // Only attach user state if include_user_state is set
                if filter.include_user_state {
                    for media in &mut records {
                        if let Some(state) = states_map.get(&media.media_key()) {
                            media.user_state = Some(state.clone());
                        }
                    }
                }
            }
        }

        Ok(FilterResult {
            records,
            total_count: if filter.total_count { count? } else { 0 },
        })
    }

    pub async fn get_refreshable(db: &SqlitePool) -> Result<Vec<Self>> {
        //           AND refreshed_at IS NULL
        let rows = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM media
        WHERE kind IN ($1, $2)

        "#,
        )
        .bind(MediaKind::Movie)
        .bind(MediaKind::Series)
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn get_by_jellyfin_filter(
        db: &sqlx::SqlitePool,
        filter: &jellyfin::GetItemsQuery,
        total_count: bool,
    ) -> Result<FilterResult<Media>> {
        let kinds = if let Some(include_item_types) = &filter.include_item_types {
            include_item_types.clone().into_vec::<MediaKind>()
        } else {
            Vec::new()
        };

        // Resolve genre names → IDs
        let genre_ids_from_names: Option<Vec<Uuid>> =
            if let Some(names) = &filter.genres {
                if names.is_empty() {
                    None
                } else {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT id FROM media WHERE kind = 'genre' AND title IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for n in names {
                        sep.push_bind(n);
                    }
                    qb.push(")");
                    let rows = qb.build().fetch_all(db).await?;
                    Some(rows.into_iter().filter_map(|r| r.get::<Option<Uuid>, _>(0)).collect())
                }
            } else {
                None
            };

        // Resolve studio names → IDs
        let studio_ids_from_names: Option<Vec<Uuid>> =
            if let Some(names) = &filter.studios {
                if names.is_empty() {
                    None
                } else {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT id FROM media WHERE kind = 'studio' AND title IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for n in names {
                        sep.push_bind(n);
                    }
                    qb.push(")");
                    let rows = qb.build().fetch_all(db).await?;
                    Some(rows.into_iter().filter_map(|r| r.get::<Option<Uuid>, _>(0)).collect())
                }
            } else {
                None
            };

        // Merge genre IDs from query param and from genre names
        let genre_ids: Option<Vec<Uuid>> = {
            let from_param: Option<Vec<Uuid>> = filter.genre_ids.as_ref().map(|ids| {
                ids.iter()
                    .flat_map(|s| s.split(','))
                    .filter_map(|s| s.trim().parse::<Uuid>().ok())
                    .collect()
            });
            match (from_param, genre_ids_from_names) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        };

        // Merge studio IDs from query param and from studio names
        let studio_ids: Option<Vec<Uuid>> = {
            let from_param: Option<Vec<Uuid>> =
                filter.studio_ids.as_ref().map(|ids| {
                    ids.iter()
                        .flat_map(|s| s.split(','))
                        .filter_map(|s| s.trim().parse::<Uuid>().ok())
                        .collect()
                });
            match (from_param, studio_ids_from_names) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        };

        let person_ids: Option<Vec<Uuid>> = filter.person_ids.as_ref().map(|ids| {
            ids.iter()
                .flat_map(|s| s.split(','))
                .filter_map(|s| s.trim().parse::<Uuid>().ok())
                .collect()
        });

        // Build user-state filter from is_favorite + filters[] items
        let item_filters = filter.filters.as_deref().unwrap_or(&[]);
        let is_played = item_filters.contains(&jellyfin::ItemFilter::IsPlayed);
        let is_unplayed = item_filters.contains(&jellyfin::ItemFilter::IsUnplayed);
        let is_resumable = item_filters.contains(&jellyfin::ItemFilter::IsResumable);
        let favorite = filter.is_favorite.or_else(|| {
            item_filters
                .contains(&jellyfin::ItemFilter::IsFavorite)
                .then_some(true)
        });

        let user_state = if favorite.is_some() || is_played || is_unplayed || is_resumable {
            Some(super::UserMediaStateFilter {
                user_id: filter.user_id,
                favorite,
                played: if is_played {
                    Some(true)
                } else if is_unplayed {
                    Some(false)
                } else {
                    None
                },
                resumable: if is_resumable { Some(true) } else { None },
                ..Default::default()
            })
        } else {
            None
        };

        Ok(Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(kinds),
                limit: filter.limit.clone(),
                id: filter.ids.clone(),
                parent_id: filter.parent_id.clone(),
                offset: filter.start_index.clone(),
                recursive: filter.recursive.unwrap_or(false),
                include_user_state: filter.enable_user_data.is_none(),
                total_count,
                user_state,
                genre_ids,
                studio_ids,
                person_ids,
                years: filter.years.clone(),
                official_ratings: filter.official_ratings.clone(),
                name_starts_with: filter.name_starts_with.clone(),
                name_starts_with_or_greater: filter
                    .name_starts_with_or_greater
                    .clone(),
                name_less_than: filter.name_less_than.clone(),
                title_contains: filter.search_term.clone(),
                index_number: filter.index_number,
                has_trailer: filter.has_trailer,
                ..Default::default()
            },
        )
        .await?)
    }

    pub async fn into_base_item(
        self,
        db: &sqlx::SqlitePool,
    ) -> Result<jellyfin::BaseItemDto> {
        //  let provider_ids = ProviderIds::get_by_media_id(db, &self.id).await?;

        let mut item = jellyfin::BaseItemDto {
            id: self.id,
            server_id: server_id(),
            type_: jellyfin::db_media_kind_to_type(self.kind.clone()),
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

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM media WHERE id = $1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn parent(&self, db: &sqlx::SqlitePool) -> Result<Option<Self>> {
        if let Some(parent_id) = &self.parent_id {
            Ok(Self::get_by_id(db, parent_id).await?)
        } else {
            Ok(None)
        }
    }

    pub async fn mark_played(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = 1;
        state.played_at = Some(Local::now().naive_local());
        state.save(db).await?;
        Ok(state)
    }

    pub async fn mark_unplayed(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = 0;
        state.played_at = None;
        state.playback_position = 0;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn mark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = true;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn unmark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = false;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn refresh_sources(
        &self,
        db: &sqlx::SqlitePool,
        aio: &aio::AioService,
    ) -> Result<Vec<Self>> {
        let kind = media_kind_to_aio(&self.kind);
        let streams = aio.get_streams(kind, self.aio_id.clone().unwrap()).await?;

        let items = streams
            .clone()
            .into_iter()
            //  .filter(|x| x.is_valid())
            .enumerate()
            .map(|(idx, stream)| {
                let mut item: Self = stream.into();
                item.parent_id = Some(self.id.clone());
                item.idx = Some(idx as i64);
                item
            })
            .collect::<Vec<Self>>();

        trace!(streams_len = streams.len(), "refreshing streams");
        Self::upsert(db, &items).await?;
        Ok(items)
    }

    pub async fn sources(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.sources.is_none() {
            let mut sources = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Source]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            sources.sort_by(|a, b| a.idx.cmp(&b.idx));

            self.sources = Some(sources);
        };
        Ok(self.sources.clone().unwrap())
    }

    pub async fn seasons(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Series {
            return Ok(vec![]);
        }

        if self.seasons.is_none() {
            let seasons = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Season]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            self.seasons = Some(seasons);
        }

        Ok(self.seasons.clone().unwrap())
    }

    pub async fn episodes(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Season {
            return Ok(vec![]);
        }

        if self.episodes.is_none() {
            let episodes = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Episode]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            self.episodes = Some(episodes);
        }

        Ok(self.episodes.clone().unwrap())
    }

    pub async fn user_state(
        &mut self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<Option<super::UserMediaState>> {
        if self.user_state.is_none() {
            let state = super::UserMediaState::get_or_new(db, user, self).await?;

            self.user_state = Some(state);
        }

        Ok(self.user_state.clone())
    }

    pub async fn load_relations(&mut self, db: &SqlitePool) -> Result<()> {
        if self.relations.is_some() {
            return Ok(());
        }

        let rels = MediaRelation::get_by_media_id(db, &self.id).await?;
        if rels.is_empty() {
            self.relations = Some(vec![]);
            return Ok(());
        }

        let media_ids: Vec<Uuid> = rels.iter().map(|r| r.right_media_id).collect();
        let related = Self::get_by_filter(
            db,
            &MediaFilter {
                id: Some(media_ids),
                ..Default::default()
            },
        )
        .await?
        .records;

        let map: std::collections::HashMap<Uuid, Media> =
            related.into_iter().map(|m| (m.id, m)).collect();

        let pairs = rels
            .into_iter()
            .filter_map(|rel| map.get(&rel.right_media_id).map(|m| (rel, m.clone())))
            .collect();

        self.relations = Some(pairs);
        Ok(())
    }

    pub fn media_key(&self) -> String {
        match self.kind {
            MediaKind::Movie | MediaKind::Series => self.imdb_id.clone().unwrap(),
            MediaKind::Season => format!(
                "{}{}",
                self.series_imdb_id.clone().unwrap(),
                self.idx.unwrap()
            ),
            MediaKind::Episode => format!(
                "{}{}{}",
                self.series_imdb_id.clone().unwrap(),
                self.parent_idx.unwrap(),
                self.idx.unwrap()
            ),
            _ => panic!("in the discoteq"),
        }
    }

    /// Count items by kind
    pub async fn count_by_kind(db: &SqlitePool, kind: &MediaKind) -> Result<i64> {
        let result = Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![kind.clone()]),
                total_count: true,
                ..Default::default()
            },
        )
        .await?;
        Ok(result.total_count as i64)
    }
}

impl From<sdks::aio::Catalog> for Media {
    fn from(source: sdks::aio::Catalog) -> Self {
        Media {
            title: source.name,
            kind: MediaKind::Collection,
            aio_id: Some(source.id.clone()),
            ..Default::default()
        }
    }
}

impl From<sdks::aio::Stream> for Media {
    fn from(source: sdks::aio::Stream) -> Self {
        Media {
            title: source.name.clone().unwrap(),
            kind: MediaKind::Source,
            url: source.url.clone(),
            id: source.get_guid(),
            ..Default::default()
        }
    }
}

impl TryFrom<sdks::aio::Meta> for Media {
    type Error = anyhow::Error;
    fn try_from(meta: sdks::aio::Meta) -> Result<Media> {
        //self.info_hash.is_some()
        // let imdb_id = meta.imdb_id.context("missing IMDB ID")?;

        let media_kind = MediaKind::from(meta.media_type.clone());
        //let imdb_id = meta.imdb_id.clone();
        //debug!(?meta.id);

        let media = Media {
            title: meta.name.unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta.released.map(|x| x.naive_utc()),
            runtime: meta.runtime.map(|d| d.num_seconds()),
            // rating_critic: meta.rating_critic,
            rating_audience: meta.imdb_rating,
            description: meta.description,
            certification: meta.certification,
            poster: meta.poster.or(meta.thumbnail),
            logo: meta.logo,
            backdrop: meta.background,
            imdb_id: meta.imdb_id.clone(),
            aio_id: meta.imdb_id.clone(),
            trailers: meta.trailers.map(|trailers| {
                sqlx::types::Json(
                    trailers
                        .into_iter()
                        .map(|t| t.source)
                        .collect::<Vec<String>>(),
                )
            }),

            //tmdb_id: Some(imdb_id.clone()),
            ..Default::default()
        };

        //if media.kind ==MediaKind::Season {
        //  media.title = format!("Season {}", media.idx.unwrap());

        //}

        // media_instances.push(media.clone());

        Ok(media)
    }
}

pub fn aio_meta_to_medias(meta: sdks::aio::Meta) -> Result<Vec<Media>> {
    let imdb_id = meta.imdb_id.clone().context("imdb_id is missing")?;

    let media: Media = meta.clone().try_into()?;

    let mut media_instances = Vec::new();
    media_instances.push(media.clone());

    if let MediaKind::Series = media.kind {
        if let Some(ref episodes) = meta.videos {
            //info!("Found {} episodes", episodes.len());
            let seasons: std::collections::BTreeMap<i64, Vec<sdks::aio::Episode>> =
                episodes
                    .iter()
                    .filter_map(|ep| ep.season.map(|s| (s, ep.clone())))
                    .fold(
                        std::collections::BTreeMap::new(),
                        |mut acc, (season, ep)| {
                            acc.entry(season).or_default().push(ep);
                            acc
                        },
                    );
            //info!("Seasons map: {:?}", seasons);
            for (season_idx, episodes) in seasons {
                let season = Media {
                    title: format!("Season {}", season_idx),
                    kind: MediaKind::Season,
                    idx: Some(season_idx),
                    series_imdb_id: media.imdb_id.clone(),
                    aio_id: Some(format!(
                        "{}:{}",
                        media.imdb_id.clone().unwrap(),
                        season_idx
                    )),
                    poster: meta.get_season_poster(season_idx),
                    parent_id: Some(media.id),
                    ..Default::default()
                };
                media_instances.push(season.clone());

                for ep in episodes {
                    let mut episode: Media = ep.clone().try_into()?;

                    episode.idx = ep.episode;
                    episode.aio_id = Some(ep.id.clone());
                    episode.series_imdb_id = media.imdb_id.clone();
                    episode.parent_id = Some(season.id);
                    episode.parent_idx = Some(season_idx);
                    media_instances.push(episode);
                }
            }
        }
    }

    Ok(media_instances)
}

pub fn collection_uuid() -> Uuid {
    uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479")
}

pub async fn ensure_collection_folder(db: &SqlitePool) -> Result<()> {
    Media {
        id: collection_uuid(),
        title: "Collections".to_string(),
        kind: MediaKind::Folder,
        promoted: 1,
        ..Default::default()
    }
    .save(db)
    .await?;
    Ok(())
}
