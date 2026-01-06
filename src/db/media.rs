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

//use crate::utils::get_uuid;
//use chrono::{DateTime, Utc};

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
pub enum MediaKind {
    #[strum(to_string = "movie")]
    Movie,
    #[strum(to_string = "series")]
    Series,
    #[strum(to_string = "season")]
    Season,
    #[strum(to_string = "episode")]
    Episode,
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
  Aio
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderIds {
    pub media_id: String,   
    pub kind: Provider,
    pub id: String,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    #[default(get_uuid())]
    pub id: String,
    pub kind: MediaKind,
    pub parent_id: Option<String>,
    pub season_num: Option<i64>,
    pub episode_num: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Invalid media: {0}")]
    ValidationError(String),
}

impl Media {
    pub fn validate(&self) -> Result<(), MediaError> {
        match self.kind {
            MediaKind::Season if self.season_num.is_none() => {
                Err(MediaError::ValidationError(
                    "Season requires a season number".to_string(),
                ))
            }
            MediaKind::Episode if self.season_num.is_none() || self.episode_num.is_none() => {
                Err(MediaError::ValidationError(
                    "Episode requires both season and episode numbers".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    pub async fn save(&mut self, db: &sqlx::SqlitePool) -> Result<(), MediaError> {
        self.validate()?;

       // let stable_id = if self.id.is_none() {
       //     Some(Self::get_stable_id(&Some(self.imdb_id.clone()), &self.season_num, &self.episode_num).to_string())
       // } else {
        //    self.id.clone()
        //};
        let updated_at = Utc::now();
        sqlx::query!(
            r#"
            INSERT INTO media (
                id, kind, parent_id, season_num, episode_num, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT (id) DO UPDATE SET
                kind = excluded.kind,
                parent_id = excluded.parent_id,
                season_num = excluded.season_num,
                episode_num = excluded.episode_num,
                updated_at = excluded.updated_at
            "#,
            self.id,
            self.kind,
            self.parent_id,
            self.season_num,
            self.episode_num,
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
                id, kind as "kind: MediaKind", parent_id,
                season_num,
                episode_num,
                created_at as "created_at: _", 
                updated_at as "updated_at: _"
            FROM media
            WHERE id = ?1
            "#,
            id
        )
        .fetch_optional(db)
        .await
    }
}

impl From<sdks::aio::Meta> for Vec<Media> {
    fn from(meta: sdks::aio::Meta) -> Self {
        let mut media_instances = Vec::new();

        //let imdb_id = meta.imdb_id.clone();
        let media_kind = MediaKind::from(meta.media_type.clone());
        let media = Media {
            kind: media_kind.clone(),
         //   imdb_id: meta.imdb_id.unwrap_or_default(),
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

                for (season_num, episodes) in seasons {
                    let season_media = Media {
                        kind: MediaKind::Season,
                      //  imdb_id: meta.imdb_id.clone().unwrap_or_default(),
                        season_num: Some(season_num),
                        ..Default::default() // Alle andere velden default
                    };
                    media_instances.push(season_media);

                    for episode in episodes {
                        let episode_media = Media {
                            kind: MediaKind::Episode,
                            //imdb_id: meta.imdb_id.clone().unwrap_or_default(),
                            season_num: Some(season_num),
                            episode_num: episode.episode,
                            ..Default::default() // Alle andere velden default
                        };
                        media_instances.push(episode_media);
                    }
                }
            }
        }

        media_instances
    }
}