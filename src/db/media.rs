use crate::jellyfin;
use crate::sdks;
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
use tracing::instrument;
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
    Catalog,
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
            _ => MediaKind::Unknown,
        }
    }
}

impl From<MediaKind> for jellyfin::MediaType {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Movie => jellyfin::MediaType::Movie,
            MediaKind::Series => jellyfin::MediaType::Series,
            MediaKind::Season => jellyfin::MediaType::Season,
            MediaKind::Episode => jellyfin::MediaType::Episode,
            MediaKind::Catalog => jellyfin::MediaType::BoxSet,
            // MediaKind::Catalog => jellyfin::MediaType::CollectionFolder,
            _ => jellyfin::MediaType::Unknown,
        }
    }
}

#[derive(
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
pub enum CatalogKind {
    Movie,
    Series,
}

impl TryFrom<String> for CatalogKind {
    type Error = strum::ParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaSource {
    pub id: String,
    pub media_id: String,
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub external_data: Option<String>,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaFilter {
    pub id: Option<String>,
    pub kind: Option<Vec<MediaKind>>,
    pub parent_id: Option<String>,
    pub imdb_id: Option<String>,
    pub aio_id: Option<String>,
    pub promoted: Option<bool>,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    #[default(get_uuid())]
    // shared
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub aio_id: Option<String>,

    // meta
    pub released_at: Option<NaiveDateTime>,
    pub runtime: Option<i64>,
    pub rating_critic: Option<i64>,
    pub rating_audience: Option<i64>,
    pub poster: Option<String>,
    pub parent_id: Option<String>,
    pub idx: Option<i64>,
    pub series_imdb_id: Option<String>,
    pub imdb_id: Option<String>,

    //#[sqlx(skip)]
    // pub seasons: Option<Vec<Media>>,

    // stream
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub remote_data: Option<String>,
    // catalog
    // #[sqlx(try_from="i32")]
    pub promoted: i64,
    //#[sqlx(try_from="String")]
    //pub catalog_kind: Option<CatalogKind>,
    pub catalog_kind: Option<String>,
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
    pub fn catalog_kind_enum(&self) -> Option<CatalogKind> {
        match self.catalog_kind.clone() {
            Some(s) => CatalogKind::try_from(s).ok(),
            None => None,
        }
    }
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
        }
    }

    pub async fn save(&mut self, db: &sqlx::SqlitePool) -> Result<()> {
        self.validate()?;
        let updated_at = Utc::now();

        sqlx::query!(
            r#"
            INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, poster, url, probe_data, promoted, catalog_kind,
                remote_data, series_imdb_id, aio_id, imdb_id, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
            ON CONFLICT (id) DO UPDATE SET
                title = excluded.title,
                kind = excluded.kind,
                parent_id = excluded.parent_id,
                idx = excluded.idx,
                released_at = excluded.released_at,
                runtime = excluded.runtime,
                rating_critic = excluded.rating_critic,
                rating_audience = excluded.rating_audience,
                poster = excluded.poster,
                url = excluded.url,
                probe_data = excluded.probe_data,
                remote_data = excluded.remote_data,
                series_imdb_id = excluded.series_imdb_id,
                imdb_id = excluded.imdb_id,
                aio_id = excluded.aio_id,
                promoted = excluded.promoted,
                updated_at = excluded.updated_at
            "#,
            self.id,
            self.title,
            self.kind,
            self.parent_id,
            self.idx,
            self.released_at,
            self.runtime,
            self.rating_critic,
            self.rating_audience,
            self.poster,
            self.url,
            self.probe_data,
            self.promoted,
            self.catalog_kind,
            self.remote_data,
            self.series_imdb_id,
            self.aio_id,
            self.imdb_id,
            self.created_at,
            updated_at
        )
        .execute(db)
        .await?;

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
                rating_critic, rating_audience, poster, url, probe_data, promoted, catalog_kind,
                remote_data, series_imdb_id, imdb_id, aio_id, created_at, updated_at
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
                    .push_bind(&item.url)
                    .push_bind(&item.probe_data)
                    .push_bind(&item.promoted)
                    .push_bind(&item.catalog_kind)
                    .push_bind(&item.remote_data)
                    .push_bind(&item.series_imdb_id)
                    .push_bind(&item.imdb_id)
                    .push_bind(&item.aio_id)
                    .push_bind(&item.created_at)
                    .push_bind(Utc::now());
            });

            query_builder.push(
                " ON CONFLICT DO UPDATE SET
                title = excluded.title,
                id = excluded.id,
                parent_id = excluded.parent_id,
                idx = excluded.idx,
                released_at = excluded.released_at,
                runtime = excluded.runtime,
                rating_critic = excluded.rating_critic,
                rating_audience = excluded.rating_audience,
                poster = excluded.poster,
                url = excluded.url,
                probe_data = excluded.probe_data,
                remote_data = excluded.remote_data,
                series_imdb_id = excluded.series_imdb_id,
                updated_at = excluded.updated_at,
                promoted = excluded.promoted",
            );

            query_builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn get_by_id(
        db: &sqlx::SqlitePool,
        id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Media,
            r#"
            SELECT * FROM media
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(db)
        .await
    }

    pub async fn get_by_filter(
        db: &sqlx::SqlitePool,
        filter: &MediaFilter,
    ) -> Result<Vec<Media>> {
        let mut query_builder =
            sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");

        if let Some(parent_id) = &filter.parent_id {
            query_builder.push(" AND parent_id = ").push_bind(parent_id);
        }

        if let Some(aio_id) = &filter.aio_id {
            query_builder.push(" AND aio_id = ").push_bind(aio_id);
        }

        if let Some(promoted) = &filter.promoted {
            query_builder.push(" AND promoted = ").push_bind(promoted);
        }

        if let Some(kinds) = &filter.kind {
            if !kinds.is_empty() {
                query_builder.push(" AND kind IN (");
                for (i, kind) in kinds.iter().enumerate() {
                    if i > 0 {
                        query_builder.push(", ");
                    }
                    query_builder.push_bind(kind);
                }
                query_builder.push(")");
            }
        }

        let query = query_builder.build_query_as::<Media>();
        Ok(query.fetch_all(db).await?)
    }

    pub async fn into_base_item(
        self,
        db: &sqlx::SqlitePool,
    ) -> Result<jellyfin::BaseItemDto> {
        //  let provider_ids = ProviderIds::get_by_media_id(db, &self.id).await?;

        let mut item = jellyfin::BaseItemDto {
            id: self.id,
            server_id: server_id(),
            type_: self.kind.clone().into(),
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

    pub async fn parent(&self, db: &sqlx::SqlitePool) -> Result<Option<Self>> {
        if let Some(parent_id) = &self.parent_id {
            Ok(Self::get_by_id(db, parent_id).await?)
        } else {
            Ok(None)
        }
    }

    //pub async fn seasons(&self, db: &sqlx::SqlitePool) -> Result<Vec<Self>> {
    //    if let Some(seasons) = self.seasons {
    //        Ok(seasons)
    //    } else {
    //        Ok(vec![])
    //    }
    //}
}

impl From<sdks::aio::Catalog> for Media {
    fn from(catalog: sdks::aio::Catalog) -> Self {
        Media {
            title: catalog.name,
            kind: MediaKind::Catalog,

            aio_id: Some(catalog.id.clone()),
            ..Default::default()
        }
    }
}

impl From<sdks::aio::Meta> for Vec<Media> {
    fn from(meta: sdks::aio::Meta) -> Vec<Media> {
        let mut media_instances = Vec::new();
        let media_kind = MediaKind::from(meta.media_type.clone());

        let media = Media {
            title: meta.name.unwrap_or_default(),
            kind: media_kind.clone(),
            //   released_at: meta.released,
            //  runtime: meta.runtime.as_secs(),
            // rating_critic: meta.rating_critic,
            //rating_audience: meta.imdb_rating,
            poster: meta.poster,
            imdb_id: Some(meta.imdb_id.clone()),
            aio_id: Some(meta.imdb_id.clone()),
            ..Default::default()
        };

        media_instances.push(media.clone());

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
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        aio_id: Some(format!(
                            "{}:{}",
                            meta.imdb_id.clone(),
                            season_idx
                        )),
                        ..Default::default()
                    };
                    media_instances.push(season_media);

                    for episode in episodes {
                        let episode_media = Media {
                            kind: MediaKind::Episode,
                            title: episode.name.unwrap_or_default(),
                            idx: episode.episode,
                            aio_id: Some(episode.id.clone()),
                            series_imdb_id: media.imdb_id.clone(),
                            // released_at: episode.released,
                            //  runtime: episode.runtime.as_secs(),
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
