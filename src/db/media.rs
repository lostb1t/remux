use super::FilterResult;
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

impl From<MediaKind> for sdks::aio::MediaType {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Movie => sdks::aio::MediaType::Movie,
            MediaKind::Series => sdks::aio::MediaType::Series,
            // MediaKind::Catalog => jellyfin::MediaType::CollectionFolder,
            _ => sdks::aio::MediaType::Unknown,
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

impl From<jellyfin::MediaType> for MediaKind {
    fn from(media_type: jellyfin::MediaType) -> Self {
        match media_type {
            jellyfin::MediaType::Movie => MediaKind::Movie,
            jellyfin::MediaType::Series => MediaKind::Series,
            _ => MediaKind::Unknown,
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
    pub total_count: bool,
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
    pub aio_id: Option<String>,

    // meta
    pub released_at: Option<NaiveDateTime>,
    // in seconds
    pub runtime: Option<i64>,
    pub rating_critic: Option<i64>,
    pub rating_audience: Option<i64>,
    pub poster: Option<String>,
    pub parent_id: Option<Uuid>,
    pub idx: Option<i64>,
    pub series_imdb_id: Option<String>,
    pub imdb_id: Option<String>,
    //pub description: Option<String>,
    #[sqlx(skip)]
    pub sources: Option<Vec<Media>>,
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
    pub fn catalog_kind_enum(&self) -> Option<MediaKind> {
        match self.catalog_kind.clone() {
            Some(s) => MediaKind::try_from(s).ok(),
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
                idx = excluded.idx,
                released_at = excluded.released_at,
                runtime = excluded.runtime,
                rating_critic = excluded.rating_critic,
                rating_audience = excluded.rating_audience,
                poster = excluded.poster,
                url = excluded.url,
                aio_id = excluded.aio_id,
                imdb_id = excluded.imdb_id,
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
        db: &sqlx::SqlitePool,
        filter: &MediaFilter,
    ) -> Result<FilterResult<Media>> {
        let mut count_qb =
            sqlx::QueryBuilder::new("SELECT COUNT(*) as count FROM media WHERE 1=1");
        let mut records_qb = sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(parent_id) = &filter.parent_id {
                qb.push(" AND parent_id = ").push_bind(parent_id);
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
        }

        if let Some(limit) = &filter.limit {
            records_qb.push(" LIMIT ").push_bind(limit);
        }

        if let Some(offset) = &filter.offset {
            records_qb.push(" OFFSET ").push_bind(offset);
        }

        let (count, records) = tokio::join!(
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

        Ok(FilterResult {
            records: records?,
            total_count: if filter.total_count { count? } else { 0 },
        })
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
        Ok(Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(kinds),
                limit: filter.limit.clone(),
                id: filter.ids.clone(),
                offset: filter.start_index.clone(),
                total_count,
                //   parent_id: Some(self.id),
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
    pub async fn refresh_sources(
        &self,
        db: &sqlx::SqlitePool,
        aio: &aio::AioService,
    ) -> Result<Vec<Self>> {
        let streams = aio
            .get_streams(self.kind.clone().into(), self.aio_id.clone().unwrap())
            .await?;

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
}

trait QueryBuilderExt<'q> {
    fn push_in<T>(&mut self, column: &str, values: &'q Vec<T>)
    where
        T: Send
            + Sync
            + for<'a> sqlx::Encode<'a, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>
            + 'q;
}

impl<'q> QueryBuilderExt<'q> for sqlx::QueryBuilder<'q, sqlx::Sqlite> {
    fn push_in<T>(&mut self, column: &str, values: &'q Vec<T>)
    where
        T: Send
            + Sync
            + for<'a> sqlx::Encode<'a, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>
            + 'q,
    {
        if values.is_empty() {
            return;
        };

        self.push(" AND ");
        self.push(column);
        self.push(" IN (");

        let mut separated = self.separated(", ");
        for v in values {
            separated.push_bind(v);
        }

        self.push(")");
    }
}

impl From<sdks::aio::Catalog> for Media {
    fn from(source: sdks::aio::Catalog) -> Self {
        Media {
            title: source.name,
            kind: MediaKind::Catalog,
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

impl TryFrom<sdks::aio::Meta> for Vec<Media> {
    type Error = anyhow::Error;
    fn try_from(meta: sdks::aio::Meta) -> Result<Vec<Media>> {
        //self.info_hash.is_some()
        let imdb_id = meta.imdb_id.context("missing IMDB ID")?;

        let mut media_instances = Vec::new();
        let media_kind = MediaKind::from(meta.media_type.clone());
        //let imdb_id = meta.imdb_id.clone();

        let media = Media {
            title: meta.name.unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta.released.map(|x| x.naive_utc()),
            runtime: meta.runtime.map(|d| d.num_seconds()),
            // rating_critic: meta.rating_critic,
            //rating_audience: meta.imdb_rating,
            poster: meta.poster,
            imdb_id: Some(imdb_id.clone()),
            aio_id: Some(imdb_id.clone()),
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
                        aio_id: Some(format!("{}:{}", imdb_id.clone(), season_idx)),
                        ..Default::default()
                    };
                    media_instances.push(season_media);

                    for episode in episodes {
                        let episode_media = Media {
                            kind: MediaKind::Episode,
                            title: episode.name.unwrap_or_default(),
                            idx: episode.episode,
                            aio_id: Some(episode.id.clone()),
                            series_imdb_id: Some(imdb_id.clone()),
                            // released_at: episode.released,
                            //  runtime: episode.runtime.as_secs(),
                            ..Default::default()
                        };
                        media_instances.push(episode_media);
                    }
                }
            }
        }

        Ok(media_instances)
    }
}
