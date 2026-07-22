use axum::response::Html;
use reqwest;

use crate::{IntoApiError, OptionExt, ResultExt};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use axum::{
    Json, Router, ServiceExt,
    body::Body,
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    middleware,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_anyhow::{ApiError, ApiResult, on_error, set_expose_errors};
use chrono::{Duration, Utc, prelude::*};
use config::{self, Config};
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{self, collections::HashMap, env, fs, path::Path, sync::Arc};
use timed;
use tower::{Layer, util::MapRequestLayer};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};
use tracing::{self, debug, instrument, warn};
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use url::Url;
use uuid::Uuid;

use crate::{AppState, common::get_uuid, db};

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
    pub created_at: Option<DateTime<Utc>>,
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
            id: header
                .device_id
                .unwrap_or_else(|| get_uuid().to_string()),
            name: header
                .device
                .unwrap_or_else(|| "Unknown Device".to_string()),
            app_name: header
                .client
                .unwrap_or_else(|| "Unknown Client".to_string()),
            app_version: header
                .version
                .unwrap_or_else(|| "1.0".to_string()),
            user_id: user
                .id
                .clone(),
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

    pub async fn get_by_id(db: &SqlitePool, device_id: &str) -> Result<Option<Self>> {
        sqlx::query_as::<_, Self>("SELECT * FROM devices WHERE id = ?1 LIMIT 1")
            .bind(device_id)
            .fetch_optional(db)
            .await
            .map_err(Into::into)
    }

    /// Get all devices, optionally filtering to those active since `since`.
    pub async fn get_all(
        db: &SqlitePool,

        active_within: Option<std::time::Duration>,
    ) -> Result<Vec<Self>> {
        let devices = if let Some(duration) = active_within {
            let since = Utc::now()
                - Duration::from_std(duration)
                    .map_err(|e| anyhow::anyhow!("invalid duration: {e}"))?;

            sqlx::query_as::<_, Self>(
                r#"

            SELECT *

            FROM devices

            WHERE last_activity_at >= ?

            ORDER BY name

            "#,
            )
            .bind(since)
            .fetch_all(db)
            .await?
        } else {
            sqlx::query_as::<_, Self>(
                r#"

            SELECT *

            FROM devices

            ORDER BY name

            "#,
            )
            .fetch_all(db)
            .await?
        };

        Ok(devices)
    }

    pub async fn delete_by_access_token(db: &SqlitePool, token: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM devices WHERE access_token = ?")
            .bind(token)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_by_id(db: &SqlitePool, device_id: &str, user_id: &Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM devices WHERE id = ? AND user_id = ?")
            .bind(device_id)
            .bind(user_id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete all devices for a user, optionally skipping one token (e.g. the caller's own).
    pub async fn delete_all_for_user(
        db: &SqlitePool,
        user_id: &Uuid,
        except_token: Option<&str>,
    ) -> Result<u64> {
        let result = if let Some(token) = except_token {
            sqlx::query("DELETE FROM devices WHERE user_id = ? AND access_token != ?")
                .bind(user_id)
                .bind(token)
                .execute(db)
                .await?
        } else {
            sqlx::query("DELETE FROM devices WHERE user_id = ?")
                .bind(user_id)
                .execute(db)
                .await?
        };
        Ok(result.rows_affected())
    }

    fn merge_runtime_metadata_from_header(&mut self, header: &JellyfinAuthHeader) {
        // Only trust metadata updates for this exact device identity.
        if let Some(device_id) = header
            .device_id
            .as_deref()
        {
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
    pub async fn touch(&self, db: &SqlitePool, remote_ip: Option<&str>) -> Result<()> {
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
    pub async fn save_capabilities(
        db: &SqlitePool,
        device_id: &str,
        caps: &crate::api::ClientCapabilitiesDto,
    ) -> Result<()> {
        sqlx::query("UPDATE devices SET capabilities = ? WHERE id = ?")
            .bind(sqlx::types::Json(caps))
            .bind(device_id)
            .execute(db)
            .await?;
        Ok(())
    }

    /// Get the stored capabilities for this device, if present.
    pub fn parsed_capabilities(&self) -> Option<crate::api::ClientCapabilitiesDto> {
        self.capabilities
            .as_ref()
            .map(|j| {
                j.0.clone()
            })
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
            .context_unauthorized("forbidden")?;

        // device_id is optional — query-param-only auth (e.g. ?token=...)
        // doesn't carry a DeviceId. The device is looked up by token alone.
        let _device_id = jfauth
            .device_id
            .as_deref();

        // Capture client IP from proxy headers or peer address.
        let remote_ip = parts
            .headers
            .get("X-Forwarded-For")
            .and_then(|v| {
                v.to_str()
                    .ok()
            })
            .and_then(|v| {
                v.split(',')
                    .next()
            })
            .map(|s| {
                s.trim()
                    .to_string()
            })
            .or_else(|| {
                parts
                    .headers
                    .get("X-Real-IP")
                    .and_then(|v| {
                        v.to_str()
                            .ok()
                    })
                    .map(|s| s.to_string())
            });

        // First try the devices table (normal session token).
        if let Some(mut device) = Device::get_by_access_token(
            &state
                .ctx
                .db,
            token,
        )
        .await?
        {
            device.merge_runtime_metadata_from_header(&jfauth);
            let _ = device
                .touch(
                    &state
                        .ctx
                        .db,
                    remote_ip.as_deref(),
                )
                .await;
            let user = db::User::get_by_id(
                &state
                    .ctx
                    .db,
                &device.user_id,
            )
            .await?
            .context_unauthorized("forbidden")?;
            tracing::Span::current().record(
                "user",
                user.username
                    .as_str(),
            );
            return Ok(AuthSession { device, user });
        }

        // Fall back to the api_keys table. API keys are admin-scoped tokens.
        let api_key = db::ApiKey::get_by_token(
            &state
                .ctx
                .db,
            token,
        )
        .await?
        .context_unauthorized("forbidden")?;

        let user = sqlx::query_as::<_, db::User>(
            "SELECT * FROM users WHERE is_admin = 1 LIMIT 1",
        )
        .fetch_optional(
            &state
                .ctx
                .db,
        )
        .await?
        .context_unauthorized("forbidden")?;

        let synthetic_device = Device {
            id: format!("apikey-{}", api_key.access_token),
            access_token: api_key.access_token,
            user_id: user.id,
            name: api_key
                .app_name
                .clone(),
            app_name: api_key.app_name,
            app_version: String::new(),
            last_activity_at: None,
            capabilities: None,
            remote_ip: None,
            created_at: None,
        };

        tracing::Span::current().record(
            "user",
            user.username
                .as_str(),
        );
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
        if !session
            .user
            .is_admin
        {
            return Err(anyhow::anyhow!("forbidden").context_unauthorized("forbidden"));
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

        let scheme = parts
            .next()
            .unwrap_or("");
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
                let unquoted = val
                    .trim()
                    .trim_matches('"')
                    .to_string();
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
            token: map
                .get("Token")
                .cloned(),
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
        if let Some(auth) = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .or_else(|| {
                parts
                    .headers
                    .get("X-Emby-Authorization")
            })
            .and_then(|v| {
                v.to_str()
                    .ok()
            })
            .and_then(|raw| JellyfinAuthHeader::from_str(raw).ok())
        {
            return Ok(auth);
        }

        // Try X-Emby / MediaBrowser token headers
        let token = parts
            .headers
            .get("X-Emby-Token")
            .or_else(|| {
                parts
                    .headers
                    .get("X-MediaBrowser-Token")
            })
            .and_then(|v| {
                v.to_str()
                    .ok()
            })
            .map(|s| s.to_string());

        if let Some(token) = token {
            return Ok(JellyfinAuthHeader {
                token: Some(token),
                ..Default::default()
            });
        }

        // Query params fallback
        if let Some(query) = parts
            .uri
            .query()
        {
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

        Ok(JellyfinAuthHeader::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000).await.unwrap();
        crate::db::migrate(&db).await.unwrap();
        db
    }

    async fn insert_device(db: &SqlitePool, user_id: Uuid, token: &str) {
        Device {
            id: Uuid::new_v4().to_string(),
            access_token: token.to_string(),
            user_id,
            name: "Test Device".to_string(),
            app_name: "Test".to_string(),
            app_version: "1.0".to_string(),
            ..Default::default()
        }
        .save(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_all_removes_every_device_for_user() {
        let db = test_db().await;
        let uid = Uuid::new_v4();

        insert_device(&db, uid, "token-a").await;
        insert_device(&db, uid, "token-b").await;

        let deleted = Device::delete_all_for_user(&db, &uid, None).await.unwrap();
        assert_eq!(deleted, 2);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE user_id = ?")
            .bind(uid)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn delete_all_except_current_token() {
        let db = test_db().await;
        let uid = Uuid::new_v4();

        insert_device(&db, uid, "token-keep").await;
        insert_device(&db, uid, "token-del-1").await;
        insert_device(&db, uid, "token-del-2").await;

        let deleted = Device::delete_all_for_user(&db, &uid, Some("token-keep")).await.unwrap();
        assert_eq!(deleted, 2);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE user_id = ?")
            .bind(uid)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn delete_all_does_not_touch_other_users() {
        let db = test_db().await;
        let uid_a = Uuid::new_v4();
        let uid_b = Uuid::new_v4();

        insert_device(&db, uid_a, "token-a").await;
        insert_device(&db, uid_b, "token-b").await;

        Device::delete_all_for_user(&db, &uid_a, None).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE user_id = ?")
            .bind(uid_b)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
