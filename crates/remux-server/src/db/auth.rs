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
use axum_anyhow::IntoApiError;
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

use crate::AppState;
use crate::db;
use crate::utils::get_uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "PascalCase")]
pub struct Device {
    pub id: String,
    pub access_token: String,
    pub user_id: Uuid,
    pub name: String,
    pub app_name: String,
    pub app_version: String,
    pub last_activity_at: Option<DateTime<Utc>>,
}

impl Device {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO devices
                (user_id, access_token, id, name, app_name, app_version)
            VALUES
                (?, ?, ?, ?, ?, ?)
            ON CONFLICT(id, user_id) DO UPDATE SET
                name = excluded.name,
                access_token = excluded.access_token,
                app_name    = excluded.app_name,
                app_version = excluded.app_version
            "#,
        )
        .bind(self.user_id)
        .bind(&self.access_token)
        .bind(&self.id)
        .bind(&self.name)
        .bind(&self.app_name)
        .bind(&self.app_version)
        .execute(db)
        .await?;

        Ok(())
    }

    pub fn new_from_header(
        header: JellyfinAuthHeader,
        user: &db::User,
    ) -> Result<Self> {
        Ok(Self {
            id: header.device_id.context("missing device id")?,
            name: header.device.context("missing device name")?,
            app_name: header.client.context("missing device name")?,
            app_version: header.version.context("missing device name")?,
            user_id: user.id.clone(),
            access_token: get_uuid().to_string(),
            ..Default::default()
        })
    }

    pub async fn get_by_access_token(
        db: &SqlitePool,
        token: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM devices
        WHERE access_token = ?1
        "#,
        )
        .bind(token)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    /// Get all devices
    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        let devices = sqlx::query_as::<_, Self>(
            r#"
            SELECT *
            FROM devices
            ORDER BY name
            "#,
        )
        .fetch_all(db)
        .await?;

        Ok(devices)
    }

    pub async fn delete_by_access_token(db: &SqlitePool, token: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM devices WHERE access_token = ?")
            .bind(token)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_by_id(db: &SqlitePool, device_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM devices WHERE id = ?")
            .bind(device_id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Update last_activity_at to now.
    pub async fn touch(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "UPDATE devices SET last_activity_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ? AND user_id = ?",
        )
        .bind(&self.id)
        .bind(self.user_id)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Load the user this device belongs to.
    pub async fn user(&self, db: &SqlitePool) -> Result<Option<db::User>> {
        db::User::get_by_id(db, &self.user_id).await
    }

    /// Get devices by user ID
    pub async fn get_by_user_id(db: &SqlitePool, user_id: &Uuid) -> Result<Vec<Self>> {
        let devices = sqlx::query_as::<_, Self>(
            r#"
            SELECT *
            FROM devices
            WHERE user_id = ?1
            ORDER BY name
            "#,
        )
        .bind(user_id)
        .fetch_all(db)
        .await?;

        Ok(devices)
    }
}

#[derive(Clone)]
pub struct AuthSession {
    pub device: Device,
    pub user: db::User,
}

//#[async_trait]
impl FromRequestParts<AppState> for AuthSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jfauth = JellyfinAuthHeader::from_request_parts(parts, state).await?;
        let token = jfauth
            .token
            .as_deref()
            .context_unauthorized("forbidden", "forbidden")?;
        // device_id is optional — query-param-only auth (e.g. ?token=...)
        // doesn't carry a DeviceId. The device is looked up by token alone.
        let _device_id = jfauth.device_id;
        // First try the devices table (normal session token).
        if let Some(device) = Device::get_by_access_token(&state.ctx.db, token).await? {
            let _ = device.touch(&state.ctx.db).await;
            let user = db::User::get_by_id(&state.ctx.db, &device.user_id)
                .await?
                .context_unauthorized("forbidden", "forbidden")?;
            return Ok(AuthSession { device, user });
        }

        // Fall back to the api_keys table. API keys are admin-scoped tokens.
        let api_key = db::ApiKey::get_by_token(&state.ctx.db, token)
            .await?
            .context_unauthorized("forbidden", "forbidden")?;

        let user = sqlx::query_as::<_, db::User>(
            "SELECT * FROM users WHERE is_admin = 1 LIMIT 1",
        )
        .fetch_optional(&state.ctx.db)
        .await?
        .context_unauthorized("forbidden", "forbidden")?;

        let synthetic_device = Device {
            id: format!("apikey-{}", api_key.access_token),
            access_token: api_key.access_token,
            user_id: user.id,
            name: api_key.app_name.clone(),
            app_name: api_key.app_name,
            app_version: String::new(),
            last_activity_at: None,
        };

        Ok(AuthSession {
            device: synthetic_device,
            user,
        })
    }
}

/// Extractor that only succeeds for admin users. Derefs to AuthSession.
pub struct AdminSession(pub AuthSession);

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = AuthSession::from_request_parts(parts, state).await?;
        if !session.user.is_admin {
            return Err(anyhow::anyhow!("forbidden")
                .context_unauthorized("forbidden", "forbidden"));
        }
        Ok(AdminSession(session))
    }
}

impl std::ops::Deref for AdminSession {
    type Target = AuthSession;
    fn deref(&self) -> &AuthSession {
        &self.0
    }
}

// todo theres also an old emby airh header. Should we support this?
#[derive(Debug, Clone, Default)]
pub struct JellyfinAuthHeader {
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub token: Option<String>,
}

impl JellyfinAuthHeader {
    fn from_str(header: &str) -> Result<Self> {
        let mut map = HashMap::new();
        let mut parts = header.splitn(2, ' ');

        let scheme = parts.next().unwrap_or("");
        let rest = match parts.next() {
            Some(r) => r,
            None => return Ok(JellyfinAuthHeader::default()),
        };

        if !scheme.eq_ignore_ascii_case("MediaBrowser")
            && !scheme.eq_ignore_ascii_case("Emby")
        {
            return Ok(JellyfinAuthHeader::default());
        }

        for item in rest.split(',') {
            let item = item.trim();
            let mut kv = item.splitn(2, '=');
            if let (Some(key), Some(val)) = (kv.next(), kv.next()) {
                let unquoted = val.trim().trim_matches('"').to_string();
                map.insert(key.to_string(), unquoted);
            }
        }

        Ok(Self {
            client: map.get("Client").cloned(),
            device: map.get("Device").cloned(),
            device_id: map.get("DeviceId").cloned(),
            version: map.get("Version").cloned(),
            token: map.get("Token").cloned(),
            ..Default::default()
        })
    }
}

impl FromRequestParts<AppState> for JellyfinAuthHeader {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 1. Standard MediaBrowser/Emby Authorization header
        if let Some(raw) = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                parts
                    .headers
                    .get("X-Emby-Authorization")
                    .and_then(|v| v.to_str().ok())
            })
        {
            return Ok(JellyfinAuthHeader::from_str(raw)?);
        }

        // 2. Bare token headers (X-Emby-Token / X-MediaBrowser-Token)
        if let Some(token) = parts
            .headers
            .get("X-Emby-Token")
            .or_else(|| parts.headers.get("X-MediaBrowser-Token"))
            .and_then(|v| v.to_str().ok())
        {
            return Ok(JellyfinAuthHeader {
                token: Some(token.to_string()),
                ..Default::default()
            });
        }

        // 3. api_key / ApiKey query parameter
        if let Some(query) = parts.uri.query() {
            for pair in query.split('&') {
                let mut kv = pair.splitn(2, '=');
                if let (Some(key), Some(val)) = (kv.next(), kv.next()) {
                    if key.eq_ignore_ascii_case("api_key")
                        || key.eq_ignore_ascii_case("apikey")
                        || key.eq_ignore_ascii_case("token")
                    {
                        return Ok(JellyfinAuthHeader {
                            token: Some(val.to_string()),
                            ..Default::default()
                        });
                    }
                }
            }
        }

        Err(anyhow::anyhow!("missing auth")
            .context_unauthorized("forbidden", "forbidden"))
    }
}
