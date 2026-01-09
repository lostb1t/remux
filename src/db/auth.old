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

use crate::AppState;
use crate::db;
use crate::utils::get_uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "PascalCase")]
pub struct Device {
    pub id: String,
    pub access_token: String,
    pub user_id: String,
    pub name: String,
    pub app_name: String,
    pub app_version: String,
}

impl Device {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO auth_devices
                (user_id, access_token, id, name, app_name, app_version)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id, user_id) DO UPDATE SET
                name = excluded.name,
                access_token = excluded.access_token,
                app_name    = excluded.app_name,
                app_version = excluded.app_version
            "#,
            self.user_id,
            self.access_token,
            self.id,
            self.name,
            self.app_name,
            self.app_version
        )
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
            access_token: get_uuid(),
            ..Default::default()
        })
    }

    pub async fn get_by_access_token(
        db: &SqlitePool,
        token: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as!(
            Self,
            r#"
            SELECT
            *
            FROM auth_devices
            WHERE access_token = ?1
            "#,
            token
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }
}

#[derive(Clone)]
pub struct AuthSession {
    pub device: Device,
    pub user: db::User,
    pub aio: crate::aio::AioService,
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
        let device_id = jfauth
            .device_id
            .as_deref()
            .context_unauthorized("forbidden", "forbidden")?;
        let device = Device::get_by_access_token(&state.db, token)
            .await?
            .context_unauthorized("forbidden", "forbidden")?;
        let user = db::User::get_by_id(&state.db, &device.user_id)
            .await?
            .context_unauthorized("forbidden", "forbidden")?;
        let aio = crate::aio::AioService::from_url(&state.config.aio_url)?;
        Ok(AuthSession { device, user, aio })
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
        let raw = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                parts
                    .headers
                    .get("X-Emby-Authorization")
                    .and_then(|v| v.to_str().ok())
            })
            .context_unauthorized("forbidden", "forbidden")?;

        Ok(JellyfinAuthHeader::from_str(raw)?)
    }
}
