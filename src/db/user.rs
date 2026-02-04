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
        sqlx::query!(
            r#"
            INSERT INTO auth_users (id, username, password_hash, aio_url)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                username      = excluded.username,
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url
            "#,
            self.id,
            self.username,
            self.password_hash,
            self.aio_url
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn save_by_username(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO auth_users (id, username, password_hash, aio_url)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(username) DO UPDATE SET
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url
            "#,
            self.id,
            self.username,
            self.password_hash,
            self.aio_url
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM auth_users
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
        FROM auth_users
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
        let mut count_qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*) as count FROM auth_users WHERE 1=1",
        );
        let mut records_qb =
            sqlx::QueryBuilder::new("SELECT * FROM auth_users WHERE 1=1");

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
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CustomData {
    pub id: String,
    // #[serde(with = "serde_json")]
    // pub data: Json
    //pub data: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserMediaInfo {
    //pub id: String,
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub is_fav: bool,
    pub playback_position: i64,
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
    pub custom_prefs: Option<HashMap<String, Option<String>>>,
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
