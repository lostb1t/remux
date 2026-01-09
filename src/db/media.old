use axum::response::Html;
use reqwest;
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
use chrono::{Duration, Utc};
use config;
use config::Config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
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
use uuid::{uuid, Uuid};
use thiserror::Error;
use crate::sdks;
use crate::utils::get_uuid;
use crate::utils::server_id;
use crate::jellyfin;

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
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

impl From<sdks::aio::MediaType> for MediaKind {
    fn from(media_type: sdks::aio::MediaType) -> Self {
        match media_type {
            sdks::aio::MediaType::Movie => MediaKind::Movie,
            sdks::aio::MediaType::Series | sdks::aio::MediaType::Tv => MediaKind::Series,
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
            _ => jellyfin::MediaType::Unknown,
        }
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

#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type
)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Imdb,
    Aio,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderIds {
    pub media_id: String,
    pub kind: Provider,
    pub id: String,
}

impl ProviderIds {
    pub async fn save(&self, db: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            INSERT INTO provider_ids (media_id, kind, id)
            VALUES ($1, $2, $3)
            ON CONFLICT (media_id, kind) DO UPDATE SET id = EXCLUDED.id
            "#,
            self.media_id,
            self.kind,
            self.id
        )
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, db: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            DELETE FROM provider_ids
            WHERE id = $1 AND kind = $2
            "#,
            self.media_id,
            self.kind
        )
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn get_by_media_id(
        db: &sqlx::SqlitePool,
        media_id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProviderIds,
            r#"
            SELECT media_id, kind as "kind: Provider", id
            FROM provider_ids
            WHERE media_id = ?
            "#,
            media_id
        )
        .fetch_optional(db)
        .await
    }

    pub async fn get_by_id(
        db: &sqlx::SqlitePool,
        kind: Provider,
        id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProviderIds,
            r#"
            SELECT media_id, kind as "kind: Provider", id
            FROM provider_ids
            WHERE id = ?1 AND kind = ?2
            "#,
            id,
            kind
        )
        .fetch_optional(db)
        .await
    }
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaFilter {
    pub id: Option<String>,
    pub kind: Option<Vec<MediaKind>>,
    pub parent_id: Option<String>,
    pub idx: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    #[default(get_uuid())]
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
    pub released_at: Option<DateTime<Utc>>,
    pub runtime: Option<Duration>,
    pub rating_critic: Option<i64>,
    pub rating_audience: Option<i64>,
    pub poster: Option<String>,
    pub parent_id: Option<String>,
    pub idx: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub url: Option<String>,
    pub probe_data: Option<String>,
    pub remote_data: Option<String>,
}

#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Invalid media: {0}")]
    ValidationError(String),
}

impl Media {
    pub fn validate(&self) -> Result<(), MediaError> {
        match self.kind {
            MediaKind::Season | MediaKind::Episode if self.idx.is_none() => {
                Err(MediaError::ValidationError(
                    format!("{:?} requires an index number", self.kind)
                ))
            }
            _ => Ok(()),
        }
    }

    pub async fn save(&mut self, db: &sqlx::SqlitePool) -> Result<(), MediaError> {
        self.validate()?;
        let updated_at = Utc::now();

        sqlx::query!(
            r#"
            INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, poster, url, probe_data,
                remote_data, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (id) DO UPDATE SET
                title = excluded.title,
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
            self.remote_data,
            self.created_at,
            updated_at
        )
        .execute(db)
        .await
        .map_err(|e| MediaError::ValidationError(e.to_string()))?;

        Ok(())
    }

    pub async fn get_by_id(db: &sqlx::SqlitePool, id: &str) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Media,
            r#"
            SELECT
                id, title, kind as "kind: MediaKind", parent_id, idx,
                runtime, rating_critic, rating_audience, poster,
                url, probe_data, remote_data,
                released_at as "released_at: _",
                created_at as "created_at: _",
                updated_at as "updated_at: _"
            FROM media
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(db)
        .await
    }

    pub async fn get_with_filter(db: &sqlx::SqlitePool, filter: &MediaFilter) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Self,
            r#"
            SELECT
                id, title, kind as "kind: MediaKind", parent_id, idx,
                runtime, rating_critic, rating_audience, poster,
                url, probe_data, remote_data,
                released_at as "released_at: _",
                created_at as "created_at: _",
                updated_at as "updated_at: _"
            FROM media
            WHERE ($1 IS NULL OR parent_id = $1)
            AND ($2 IS NULL OR kind = ANY($2))
            "#,
            filter.parent_id,
            filter.kind.as_ref().map(|kinds| kinds.iter().map(|k| k.to_string()).collect::<Vec<_>>())
        )
        .fetch_all(db)
        .await
    }

    pub async fn into_base_item(
        self,
        db: &sqlx::SqlitePool,
    ) -> Result<jellyfin::BaseItemDto> {
        let provider_ids = ProviderIds::get_by_media_id(db, &self.id).await?;

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
}

impl From<sdks::aio::Meta> for Vec<Media> {
    fn from(meta: sdks::aio::Meta) -> Self {
        let mut media_instances = Vec::new();
        let media_kind = MediaKind::from(meta.media_type.clone());

        let media = Media {
            title: meta.name.unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta.released,
            runtime: meta.runtime,
           // rating_critic: meta.rating_critic,
            rating_audience: meta.imdb_rating,
            poster: meta.poster,
            ..Default::default()
        };
        media_instances.push(media);

        if let MediaKind::Series = media_kind {
            if let Some(episodes) = meta.videos {
                let seasons: std::collections::BTreeMap<i64, Vec<sdks::aio::Episode>> =
                    episodes.into_iter()
                        .filter_map(|ep| ep.season.map(|s| (s, ep)))
                        .fold(std::collections::BTreeMap::new(), |mut acc, (season, ep)| {
                            acc.entry(season).or_default().push(ep);
                            acc
                        });

                for (season_idx, episodes) in seasons {
                    let season_media = Media {
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        ..Default::default()
                    };
                    media_instances.push(season_media);

                    for episode in episodes {
                        let episode_media = Media {
                            kind: MediaKind::Episode,
                            title: episode.name.unwrap_or_default(),
                            idx: episode.episode,
                            released_at: episode.released,
                            runtime: episode.runtime,
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