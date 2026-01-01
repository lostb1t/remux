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
use uuid::Uuid;
use argon2::{
    password_hash::{
        rand_core::OsRng,
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString
    },
    Argon2
};

use crate::utils::get_uuid;
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaSource {
    pub id: String,   
    pub media_id: String,
    pub url: Option<String>,
    pub probe_data: Option<String>,
   // pub aio_id: String,
   // pub aio_meta: Option<String>,
   // pub aio_stream: Option<String>,
   // pub created_at: DateTime<Utc>,
    pub external_data: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    pub id: String,   
    pub kind: MediaKind,
    pub parent_id: Option<String>,
    //pub url: Option<String>,
    pub imdb_id: Option<String>,
    pub aio_id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub probe_data: Option<String>,

    //pub aio_stream_id: String,
    pub aio_meta: Option<String>,
   // pub aio_stream: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Media {
    /// Save the media to the database.
    /// If the media already exists (same ID), update it.
    pub async fn save(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO media (
                id, kind, parent_id, url, imdb_id, season, episode,
                probe_data, aio_id, aio_meta, aio_stream, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                parent_id = excluded.parent_id,
                url = excluded.url,
                imdb_id = excluded.imdb_id,
                season = excluded.season,
                episode = excluded.episode,
                probe_data = excluded.probe_data,
                aio_id = excluded.aio_id,
                aio_meta = excluded.aio_meta,
                aio_stream = excluded.aio_stream,
                updated_at = excluded.updated_at
            "#,
            self.id,
            self.kind.to_string(),
            self.parent_id,
            self.url,
            self.imdb_id,
            self.season,
            self.episode,
            self.probe_data,
            self.aio_id,
            self.aio_meta,
            self.aio_stream,
            self.created_at,
            Utc::now() // Update the `updated_at` timestamp
        )
        .execute(db)
        .await?;

        Ok(())
    }

    /// Fetch a media item by its ID.
    pub async fn get_by_id(db: &SqlitePool, id: &str) -> Result<Option<Self>> {
        let row = sqlx::query_as!(
            Media,
            r#"
            SELECT
                id, kind as "kind: MediaKind", parent_id, url, imdb_id,
                season, episode, probe_data, aio_id, aio_meta, aio_stream,
                created_at, updated_at
            FROM media
            WHERE id = ?1
            "#,
            id
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }
    
    pub fn generate_deterministic_id(
        kind: &MediaKind,
        imdb_id: &Option<String>,
        url: &Option<String>,
        season: &Option<i64>,
        episode: &Option<i64>,
    ) -> Uuid {
        let namespace = uuid!("00000000-0000-0000-0000-000000000000");

        let mut input = kind.to_string();
        if let Some(imdb_id) = imdb_id {
            input.push_str(&format!(":{}", imdb_id));
        }
        if let Some(season) = season {
            input.push_str(&format!(":{}", season));
        }
        if let Some(episode) = episode {
            input.push_str(&format!(":{}", episode));
        }
        if let Some(url) = url {
            input.push_str(&format!(":{}", url));
        }
        // Generate the deterministic UUID
        Uuid::new_v5(&namespace, input.as_bytes())
    }
}