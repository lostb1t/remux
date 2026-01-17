use axum::response::Html;
use reqwest;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip_serializing)]
    pub aio_url: Option<String>,
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
        "#
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
pub struct UserMediaInfo {
    //pub id: String,
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub is_fav: bool,
    pub playback_position: i64,
}
