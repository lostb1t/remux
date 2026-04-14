use super::{FilterResult, QueryBuilderExt};
use crate::sdks;
use crate::utils::get_uuid;
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserMediaState {
    pub user_id: Uuid,
    pub stream_id: Option<Uuid>,
    pub media_key: String,
    pub favorite: bool,
    pub play_count: i64,
    pub played_at: Option<NaiveDateTime>,
    pub playback_position: i64,
    pub last_played_at: Option<NaiveDateTime>,
    //pub stream_id: Option<Uuid>,
    pub subtitle_idx: Option<i64>,
    pub audio_idx: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMediaStateFilter {
    pub user_id: Option<Uuid>,
    pub media_key: Option<Vec<String>>,
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
            r#"
        SELECT *
        FROM user_media_state
        WHERE user_id = ?1 AND media_key = ?2
        "#,
        )
        .bind(user.id)
        .bind(media.media_id.clone())
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_or_new(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
    ) -> Result<Self> {
        let row = Self::get_by_user_and_media(db, user, media).await?;
        let media_key = media
            .media_id
            .clone()
            .unwrap_or_else(|| media.id.as_simple().to_string());
        Ok(row.unwrap_or_else(|| Self {
            user_id: user.id,
            media_key,
            ..Default::default()
        }))
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        debug!(
            "Saving user media state for user {} and media key {}",
            self.user_id, self.media_key
        );

        let now = chrono::Utc::now().naive_utc();
        sqlx::query(
            r#"
            INSERT INTO user_media_state (
                user_id,
                stream_id,
                media_key,
                favorite,
                play_count,
                played_at,
                playback_position,
                last_played_at,
                subtitle_idx,
                audio_idx
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(user_id, media_key)
            DO UPDATE SET
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
        .bind(self.stream_id)
        .bind(&self.media_key)
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
            if let Some(media_keys) = &filter.media_key {
                qb.push_in("media_key", &media_keys);
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
    #[default("Horizontal".to_string())]
    pub scroll_direction: String,
    #[default(true)]
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    #[default("Ascending".to_string())]
    pub sort_order: String,
    pub show_sidebar: bool,
    pub home_sections: Option<Vec<HomeSection>>,
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
