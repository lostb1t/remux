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
    pub capabilities: Option<sqlx::types::Json<crate::api::ClientCapabilitiesDto>>,
    pub remote_ip: Option<String>,
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

    fn merge_runtime_metadata_from_header(
        &mut self,
        header: &JellyfinAuthHeader,
    ) {
        // Only trust metadata updates for this exact device identity.
        if let Some(device_id) = header.device_id.as_deref() {
            if device_id != self.id {
                return;
            }
        }

        if let Some(device_name) = header
            .device
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            self.name = device_name.to_string();
        }
        if let Some(client_name) = header
            .client
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            self.app_name = client_name.to_string();
        }
        if let Some(client_version) = header
            .version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            self.app_version = client_version.to_string();
        }
    }

    /// Update last_activity_at to now and refresh runtime-identifying fields
    /// (device name/client/version) when present.
    pub async fn touch(
        &self,
        db: &SqlitePool,
        remote_ip: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE devices SET last_activity_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), \
             remote_ip = COALESCE(?, remote_ip), \
             name = COALESCE(NULLIF(?, ''), name), \
             app_name = COALESCE(NULLIF(?, ''), app_name), \
             app_version = COALESCE(NULLIF(?, ''), app_version) \
             WHERE id = ? AND user_id = ?",
        )
        .bind(remote_ip)
        .bind(&self.name)
        .bind(&self.app_name)
        .bind(&self.app_version)
        .bind(&self.id)
        .bind(self.user_id)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Store client capabilities JSON for this device.
    pub async fn save_capabilities(db: &SqlitePool, device_id: &str, caps: &crate::api::ClientCapabilitiesDto) -> Result<()> {
        sqlx::query("UPDATE devices SET capabilities = ? WHERE id = ?")
            .bind(sqlx::types::Json(caps))
            .bind(device_id)
            .execute(db)
            .await?;
        Ok(())
    }

    /// Get the stored capabilities for this device, if present.
    pub fn parsed_capabilities(&self) -> Option<crate::api::ClientCapabilitiesDto> {
        self.capabilities.as_ref().map(|j| j.0.clone())
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
        let _device_id = jfauth.device_id.as_deref();

        // Capture client IP from proxy headers or peer address.
        let remote_ip = parts
            .headers
            .get("X-Forwarded-For")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                parts
                    .headers
                    .get("X-Real-IP")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            });

        // First try the devices table (normal session token).
        if let Some(mut device) = Device::get_by_access_token(&state.ctx.db, token).await? {
            device.merge_runtime_metadata_from_header(&jfauth);
            let _ = device.touch(&state.ctx.db, remote_ip.as_deref()).await;
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
            capabilities: None,
            remote_ip: None,
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
    fn decode_header_text(value: &str) -> String {
        // Some clients send header values percent-encoded (for example
        // Jellyfin%20Web). Decode once so active-session labels are readable.
        let wrapped = format!("v={value}");
        url::form_urlencoded::parse(wrapped.as_bytes())
            .find_map(|(k, v)| if k == "v" { Some(v.into_owned()) } else { None })
            .unwrap_or_else(|| value.to_string())
    }

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
            client: map
                .get("Client")
                .map(|v| Self::decode_header_text(v)),
            device: map
                .get("Device")
                .map(|v| Self::decode_header_text(v)),
            device_id: map
                .get("DeviceId")
                .map(|v| Self::decode_header_text(v)),
            version: map
                .get("Version")
                .map(|v| Self::decode_header_text(v)),
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
