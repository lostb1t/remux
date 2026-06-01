use super::{FilterResult, QueryBuilderExt};
use crate::api::{ScrollDirection, SortOrder};
use crate::common::get_uuid;
use crate::sdks;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use argon2::{
    Argon2,
    password_hash::{
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
    },
};
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
use chrono::{Duration, Utc};
use config;
use config::Config;
use default2;
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip_serializing)]
    pub aio_url: Option<String>,
    pub configuration: Option<sqlx::types::Json<crate::api::UserConfiguration>>,
    pub is_admin: bool,
    pub policy: Option<sqlx::types::Json<crate::api::UserPolicy>>,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserFilter {
    pub id: Option<Vec<Uuid>>,
    pub username: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub total_count: bool,
}

impl User {
    pub async fn save(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, username, password_hash, aio_url, configuration, is_admin, policy)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                username      = excluded.username,
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url,
                configuration = excluded.configuration,
                is_admin      = excluded.is_admin,
                policy        = excluded.policy
            "#,
        )
        .bind(self.id)
        .bind(&self.username)
        .bind(&self.password_hash)
        .bind(&self.aio_url)
        .bind(&self.configuration)
        .bind(self.is_admin)
        .bind(&self.policy)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn save_by_username(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, username, password_hash, aio_url, configuration, is_admin, policy)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(username) DO UPDATE SET
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url,
                is_admin      = excluded.is_admin
            "#,
        )
        .bind(self.id)
        .bind(&self.username)
        .bind(&self.password_hash)
        .bind(&self.aio_url)
        .bind(&self.configuration)
        .bind(self.is_admin)
        .bind(&self.policy)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn save_configuration(
        db: &SqlitePool,
        id: &Uuid,
        config: &crate::api::UserConfiguration,
    ) -> Result<()> {
        let json = sqlx::types::Json(config.clone());
        sqlx::query(r#"UPDATE users SET configuration = ?1 WHERE id = ?2"#)
            .bind(&json)
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM users
        WHERE id = ?1
        "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_by_username(
        db: &SqlitePool,
        username: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM users
        WHERE username = ?1
        "#,
        )
        .bind(username)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub fn new_with_password(
        key: String,
        username: String,
        password: &str,
        aio_url: Option<String>,
    ) -> Result<Self> {
        let password_hash = Self::hash_password(password)?;
        Ok(Self {
            id: get_uuid(),
            username,
            password_hash,
            aio_url,
            ..Default::default()
        })
    }

    pub async fn get_by_filter(
        db: &sqlx::SqlitePool,
        filter: &UserFilter,
    ) -> Result<FilterResult<User>> {
        let mut count_qb =
            sqlx::QueryBuilder::new("SELECT COUNT(*) as count FROM users WHERE 1=1");
        let mut records_qb = sqlx::QueryBuilder::new("SELECT * FROM users WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }
            if let Some(username) = &filter.username {
                qb.push(" AND username = ").push_bind(username);
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
                let query = records_qb.build_query_as::<User>();
                query.fetch_all(db).await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: if filter.total_count { count? } else { 0 },
        })
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        self.password_hash = Self::hash_password(password)?;
        Ok(())
    }

    pub fn verify_password(&self, password: &str) -> Result<bool> {
        let parsed = PasswordHash::new(&self.password_hash)
            .map_err(|e| anyhow!("invalid stored password hash: {e}"))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }

    pub fn hash_password(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("password hashing failed: {e}"))?;

        Ok(hash.to_string())
    }

    pub async fn authenticate(
        db: &SqlitePool,
        username: &str,
        password: &str,
    ) -> Result<Option<Self>> {
        let Some(user) = Self::get_by_username(db, username).await? else {
            return Ok(None);
        };

        if user.verify_password(password)? {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_media_state(
        &self,
        db: &SqlitePool,
        media: &super::Media,
    ) -> Result<Option<UserMediaState>> {
        Ok(UserMediaState::get_by_user_and_media(db, self, media).await?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CustomData {
    pub id: String,
    // #[serde(with = "serde_json")]
    // pub data: Json
    //pub data: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaIdRaw {
    pub kind: super::MediaKind,
    pub external_ids: super::ExternalIds,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl MediaIdRaw {
    pub fn canonical(&self) -> Option<String> {
        use super::MediaKind;
        match self.kind {
            MediaKind::Movie | MediaKind::Series | MediaKind::TvProgram => {
                self.external_ids.imdb.clone()
            }
            MediaKind::Season => {
                let series_imdb = self.external_ids.series_imdb.as_deref()?;
                Some(format!("{}:{}", series_imdb, self.season.unwrap_or(0)))
            }
            MediaKind::Episode => {
                let series_imdb = self.external_ids.series_imdb.as_deref()?;
                Some(format!(
                    "{}:{}:{}",
                    series_imdb,
                    self.season.unwrap_or(0),
                    self.episode.unwrap_or(0)
                ))
            }
            MediaKind::Artist => {
                self.external_ids.deezer_artist.map(|id| id.to_string())
            }
            MediaKind::Album => self.external_ids.deezer_album.map(|id| id.to_string()),
            MediaKind::Track => self.external_ids.deezer_track.map(|id| id.to_string()),
            MediaKind::Person => self.external_ids.tmdb.map(|id| id.to_string()),
            _ => None,
        }
    }
}

impl From<&MediaIdRaw> for Uuid {
    fn from(raw: &MediaIdRaw) -> Uuid {
        crate::common::stable_media_uuid(
            &raw.kind,
            &raw.canonical().unwrap_or_default(),
        )
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserMediaState {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub media_raw: Option<String>,
    pub stream_id: Option<Uuid>,
    pub favorite: bool,
    pub play_count: i64,
    pub played_at: Option<NaiveDateTime>,
    pub playback_position: i64,
    pub last_played_at: Option<NaiveDateTime>,
    pub subtitle_idx: Option<i64>,
    pub audio_idx: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMediaStateFilter {
    pub user_id: Option<Uuid>,
    pub media_id: Option<Vec<Uuid>>,
    pub played: Option<bool>,
    pub favorite: Option<bool>,
    pub resumable: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl UserMediaState {
    pub async fn get_by_user_and_media(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(user.id)
        .bind(media.id)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_or_new(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
    ) -> Result<Self> {
        if let Some(row) = Self::get_by_user_and_media(db, user, media).await? {
            return Ok(row);
        }

        let raw = media.media_id_raw();

        // Content-based fallback: catches legacy rows stored under a different UUID
        // (e.g. pre-fix random UUID) for the same content, matched via media_raw JSON.
        let fallback: Option<Self> = match media.kind {
            super::MediaKind::Movie | super::MediaKind::Series => {
                if let Some(imdb) = &raw.external_ids.imdb {
                    sqlx::query_as(
                        "SELECT * FROM user_media_state \
                         WHERE user_id = ? \
                           AND json_valid(media_raw) \
                           AND json_extract(media_raw, '$.kind') = ? \
                           AND json_extract(media_raw, '$.external_ids.imdb') = ? \
                         LIMIT 1",
                    )
                    .bind(user.id)
                    .bind(media.kind.to_string())
                    .bind(imdb)
                    .fetch_optional(db)
                    .await?
                } else {
                    None
                }
            }
            super::MediaKind::Season => {
                if let (Some(series_imdb), Some(season)) =
                    (&raw.external_ids.series_imdb, raw.season)
                {
                    sqlx::query_as(
                        "SELECT * FROM user_media_state \
                         WHERE user_id = ? \
                           AND json_valid(media_raw) \
                           AND json_extract(media_raw, '$.kind') = ? \
                           AND json_extract(media_raw, '$.external_ids.series_imdb') = ? \
                           AND json_extract(media_raw, '$.season') = ? \
                         LIMIT 1",
                    )
                    .bind(user.id)
                    .bind(media.kind.to_string())
                    .bind(series_imdb)
                    .bind(season)
                    .fetch_optional(db)
                    .await?
                } else {
                    None
                }
            }
            super::MediaKind::Episode => {
                if let (Some(series_imdb), Some(season), Some(episode)) =
                    (&raw.external_ids.series_imdb, raw.season, raw.episode)
                {
                    sqlx::query_as(
                        "SELECT * FROM user_media_state \
                         WHERE user_id = ? \
                           AND json_valid(media_raw) \
                           AND json_extract(media_raw, '$.kind') = ? \
                           AND json_extract(media_raw, '$.external_ids.series_imdb') = ? \
                           AND json_extract(media_raw, '$.season') = ? \
                           AND json_extract(media_raw, '$.episode') = ? \
                         LIMIT 1",
                    )
                    .bind(user.id)
                    .bind(media.kind.to_string())
                    .bind(series_imdb)
                    .bind(season)
                    .bind(episode)
                    .fetch_optional(db)
                    .await?
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(row) = fallback {
            return Ok(row);
        }

        Ok(Self {
            user_id: user.id,
            media_id: media.id,
            media_raw: serde_json::to_string(&raw).ok(),
            ..Default::default()
        })
    }

    /// Persist playback position (and optionally stream-selection preferences)
    /// for a user/media pair.
    ///
    /// * `position_ticks` – current playback position in 100-nanosecond ticks.
    /// * `audio_idx` / `subtitle_idx` – stream selections to remember; pass
    ///   `None` to leave existing values unchanged.
    /// * `runtime_seconds` – when `Some`, the 90 % "mark as watched" threshold
    ///   is applied. Pass `None` for progress updates (no watched-check) and
    ///   `Some(media.runtime)` for stop events.
    pub async fn update_playback(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
        position_ticks: i64,
        audio_idx: Option<i64>,
        subtitle_idx: Option<i64>,
        runtime_seconds: Option<i64>,
    ) -> Result<()> {
        let mut ms = Self::get_or_new(db, user, media).await?;
        let position_seconds = position_ticks / 10_000_000;
        ms.playback_position = position_seconds;

        if let Some(idx) = audio_idx {
            ms.audio_idx = Some(idx);
        }
        if let Some(idx) = subtitle_idx {
            ms.subtitle_idx = Some(idx);
        }

        // Persist the position + stream preferences first.
        ms.save(db).await?;

        // Apply the "mark as watched" threshold only on stop events.
        // Delegate to `media.mark_played` so that finishing an episode also
        // propagates to the parent season / series.
        if let Some(runtime) = runtime_seconds {
            if runtime > 0 && position_seconds >= (runtime * 90 / 100) {
                media.mark_played(db, user, true).await?;
                // Reset playback position now that the item is fully watched.
                sqlx::query(
                    "UPDATE user_media_state SET playback_position = 0 \
                     WHERE user_id = ? AND media_id = ?",
                )
                .bind(user.id)
                .bind(media.id)
                .execute(db)
                .await?;
            }
        }

        Ok(())
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        debug!(
            "Saving user media state for user {} and media_id {}",
            self.user_id, self.media_id
        );

        let now = chrono::Utc::now().naive_utc();
        sqlx::query(
            r#"
            INSERT INTO user_media_state (
                user_id,
                media_id,
                media_raw,
                stream_id,
                favorite,
                play_count,
                played_at,
                playback_position,
                last_played_at,
                subtitle_idx,
                audio_idx
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(user_id, media_id)
            DO UPDATE SET
                media_raw = excluded.media_raw,
                stream_id = excluded.stream_id,
                favorite = excluded.favorite,
                play_count = excluded.play_count,
                played_at = excluded.played_at,
                playback_position = excluded.playback_position,
                last_played_at = excluded.last_played_at,
                subtitle_idx = excluded.subtitle_idx,
                audio_idx = excluded.audio_idx
            "#,
        )
        .bind(self.user_id)
        .bind(self.media_id)
        .bind(&self.media_raw)
        .bind(self.stream_id)
        .bind(self.favorite)
        .bind(self.play_count)
        .bind(self.played_at)
        .bind(self.playback_position)
        .bind(now)
        .bind(self.subtitle_idx)
        .bind(self.audio_idx)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_filter(
        db: &SqlitePool,
        filter: &UserMediaStateFilter,
    ) -> Result<FilterResult<Self>> {
        let mut count_qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*) as count FROM user_media_state WHERE 1=1",
        );
        let mut records_qb =
            sqlx::QueryBuilder::new("SELECT * FROM user_media_state WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(user_id) = &filter.user_id {
                qb.push(" AND user_id = ").push_bind(user_id);
            }
            if let Some(media_ids) = &filter.media_id {
                qb.push_in("media_id", &media_ids);
            }
            if let Some(played) = &filter.played {
                qb.push(" AND play_count > 0");
            }
            if let Some(favorite) = &filter.favorite {
                qb.push(" AND favorite = ").push_bind(favorite);
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
                let query = records_qb.build_query_as::<UserMediaState>();
                query.fetch_all(db).await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: count?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HomeSection {
    pub order: i64,
    pub kind: String,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefsData {
    pub view_type: Option<String>,
    pub sort_by: Option<String>,
    pub index_by: Option<String>,
    #[default(false)]
    pub remember_indexing: bool,
    #[default(250)]
    pub primary_image_height: i64,
    #[default(250)]
    pub primary_image_width: i64,
    #[serde(default)]
    pub custom_prefs: HashMap<String, Option<String>>,
    #[default(ScrollDirection::Horizontal)]
    pub scroll_direction: ScrollDirection,
    #[default(true)]
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    #[default(SortOrder::Ascending)]
    pub sort_order: SortOrder,
    pub show_sidebar: bool,
    pub home_sections: Option<Vec<HomeSection>>,
}

pub fn default_homescreen_custom_prefs() -> HashMap<String, Option<String>> {
    [
        ("homesection0", "smalllibrarytiles"),
        ("homesection1", "resume"),
        ("homesection2", "nextup"),
        ("homesection3", "latestmedia"),
        ("homesection4", "livetv"),
        ("homesection5", "none"),
        ("homesection6", "none"),
        ("homesection7", "none"),
        ("homesection8", "none"),
        ("homesection9", "none"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
    .collect()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefs {
    pub id: String,
    pub user_id: Uuid,
    pub client: Option<String>,
    pub data: sqlx::types::Json<JellyfinDisplayPrefsData>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefsFilter {
    pub id: Option<Vec<String>>,
    pub user_id: Option<Uuid>,
    pub client: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub total_count: bool,
}

impl JellyfinDisplayPrefs {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jellyfin_display_prefs (id, user_id, client, data)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                user_id = excluded.user_id,
                client  = excluded.client,
                data    = excluded.data
            "#,
        )
        .bind(&self.id)
        .bind(self.user_id)
        .bind(&self.client)
        .bind(&self.data)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_filter(
        db: &sqlx::SqlitePool,
        filter: &JellyfinDisplayPrefsFilter,
    ) -> Result<FilterResult<Self>> {
        let mut count_qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*) as count FROM jellyfin_display_prefs WHERE 1=1",
        );
        let mut records_qb =
            sqlx::QueryBuilder::new("SELECT * FROM jellyfin_display_prefs WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }
            if let Some(client) = &filter.client {
                qb.push(" AND client = ").push_bind(client);
            }
            if let Some(user_id) = &filter.user_id {
                qb.push(" AND user_id = ").push_bind(user_id);
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
                let query = records_qb.build_query_as::<Self>();
                query.fetch_all(db).await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: if filter.total_count { count? } else { 0 },
        })
    }
}
