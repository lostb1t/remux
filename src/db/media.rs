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
            jellyfin::MediaType::Season => MediaKind::Season,
            jellyfin::MediaType::Episode => MediaKind::Episode,
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
pub enum CatalogKind {
    #[default]
    Manual,
    Smart,
}

impl TryFrom<String> for CatalogKind {
    type Error = strum::ParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaRelations {
    #[default(get_uuid())]
    pub id: Uuid,
    pub left_media_id: Uuid,
    pub right_media_id: Uuid,
    pub weight: Option<i64>,
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
    pub include_user_state: bool,
    pub user_state: Option<super::UserMediaStateFilter>,
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
    pub user_state: Option<super::UserMediaState>,
    // pub seasons: Option<Vec<Media>>,

    // stream
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub remote_data: Option<String>,

    // catalog
    pub promoted: i64,
    // CatalogKind
    pub catalog_kind: Option<CatalogKind>,
    // MediaKind
    pub catalog_media_kind: Option<MediaKind>,
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
        let updated_at = Utc::now();

        sqlx::query!(
        r#"
        INSERT INTO media (
            id, title, kind, parent_id, idx, released_at, runtime,
            rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, catalog_kind, catalog_media_kind,
            remote_data, series_imdb_id, aio_id, imdb_id, created_at, updated_at, certification, parent_idx
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27)
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
            updated_at = excluded.updated_at,
            certification = excluded.certification,
            parent_idx = excluded.parent_idx
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
        self.logo,
        self.backdrop,
        self.description,
        self.trailers,
        self.url,
        self.probe_data,
        self.promoted,
        self.catalog_kind,
        self.catalog_media_kind,
        self.remote_data,
        self.series_imdb_id,
        self.aio_id,
        self.imdb_id,
        self.created_at,
        updated_at,
        self.certification,
        self.parent_idx,
    )
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
                rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, catalog_kind, catalog_media_kind,
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
                    .push_bind(&item.catalog_kind)
                    .push_bind(&item.catalog_media_kind)
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
                rating_critic, rating_audience, poster, logo, backdrop, description, trailers, url, probe_data, promoted, catalog_kind, catalog_media_kind,
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
                    .push_bind(&item.catalog_kind)
                    .push_bind(&item.catalog_media_kind)
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
                id = excluded.id,
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
                parent_idx = excluded.parent_idx",
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
        db: &SqlitePool,
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
            if let Some(imdb_id) = &filter.imdb_id {
                qb.push(" AND imdb_id = ").push_bind(imdb_id);
            }

            if let Some(user_state_filter) = &filter.user_state {
                // Join with user_media_state table for filtering
                qb.push(" AND EXISTS (")
                    .push("SELECT 1 FROM user_media_state ums ")
                    .push("WHERE ums.media_key = media.aio_id ");
                
                if let Some(user_id) = &user_state_filter.user_id {
                    qb.push("AND ums.user_id = ").push_bind(user_id);
                }
                
                if let Some(played) = &user_state_filter.played {
                    if *played {
                        qb.push(" AND ums.play_count > 0");
                    } else {
                        qb.push(" AND ums.play_count = 0");
                    }
                }
                
                if let Some(favorite) = &user_state_filter.favorite {
                    qb.push(" AND ums.favorite = ").push_bind(favorite);
                }
                
                qb.push(")");
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
                parent_id: filter.parent_id.clone(),
                offset: filter.start_index.clone(),
                include_user_state: filter.enable_user_data.is_none(),
                total_count,
                user_state: {
                    let favorite = filter.is_favorite.or_else(|| {
                        filter.filters.as_ref().and_then(|f| {
                            f.contains(&jellyfin::ItemFilter::IsFavorite)
                                .then_some(true)
                        })
                    });
                    if favorite.is_some() {
                        Some(super::UserMediaStateFilter {
                            user_id: filter.user_id,
                            favorite: favorite.clone(),
                            ..Default::default()
                        })
                    } else {
                        None
                    }
                },
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
        let kind = match &self.kind {
            MediaKind::Movie => MediaKind::Movie.into(),
            _ => MediaKind::Series.into(),
        };
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

        // media_instances.push(media.clone());

        Ok(media)
    }
}

impl TryFrom<sdks::aio::Meta> for Vec<Media> {
    type Error = anyhow::Error;
    fn try_from(meta: sdks::aio::Meta) -> Result<Vec<Media>> {
        let imdb_id = meta.imdb_id.clone().context("imdb_id is missing")?;

        let media: Media = meta.clone().try_into()?;

        let mut media_instances = Vec::new();
        media_instances.push(media.clone());

        if let MediaKind::Series = media.kind {
            if let Some(episodes) = meta.videos {
                let seasons: std::collections::BTreeMap<i64, Vec<sdks::aio::Meta>> =
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
                    let season = Media {
                        title: format!("Season {}", season_idx),
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        //series_imdb_id: media.imdb_id.clone(),
                        aio_id: Some(format!(
                            "{}:{}",
                            media.imdb_id.clone().unwrap(),
                            season_idx
                        )),
                        poster: meta
                            .season_posters
                            .as_ref()
                            .and_then(|posters| posters.get(season_idx as usize))
                            .cloned(),
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

                        media_instances.push(episode);
                    }
                }
            }
        }

        Ok(media_instances)
    }
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
