use super::DbConn;
use anyhow::{Context, Result};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum_anyhow::{ApiError, ApiResult};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use diesel_enum::DbEnum;
use uuid::Uuid;
use super::schema;
use super::schema::{auth_devices};
use diesel_enum::DbEnum;

#[derive(Debug, Clone, Queryable, Insertable, Serialize, Deserialize)]
#[diesel(table_name = super::schema::auth_devices)]
#[serde(rename_all = "PascalCase")]
pub struct Device {
    pub id: String,
    pub access_token: String,
    pub user_id: String,
    pub name: String,
    pub app_name: String,
    pub app_version: String,
}
u
impl Device {
    pub fn save(&self, pool: &DbConn) -> Result<usize> {
        let mut conn = pool.get_conn()?;
        Ok(diesel::insert_into(auth_devices::table)
            .values(self)
            .on_conflict((auth_devices::id, auth_devices::user_id))
            .do_update()
            .set(self)
            .execute(conn)?)
    }

    pub fn get_by_access_token(
        pool: &DbConn,
        token: &str,
    ) -> Result<Option<Self>> {
        let mut conn = pool.get_conn()?;
        Ok(auth_devices::table
            .filter(auth_devices::access_token.eq(token))
            .first::<Self>(&mut conn)
            .optional()?)
    }

    pub fn new_from_header(
        header: JellyfinAuthHeader,
        user: &crate::db::User,
    ) -> ApiResult<Self> {
        Ok(Self {
            id: header.device_id.context("missing device id")?,
            name: header.device.context("missing device name")?,
            app_name: header.client.context("missing client name")?,
            app_version: header.version.context("missing version")?,
            user_id: user.id.clone(),
            access_token: Uuid::new_v4().to_string(),
        })
    }
}

#[derive(Clone)]
pub struct AuthSession {
    pub device: Device,
    pub user: crate::db::User,
    pub aio: crate::aio::AioService,
}

impl FromRequestParts<crate::AppState> for AuthSession {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> ApiResult<Self> {
        let jfauth = JellyfinAuthHeader::from_request_parts(parts, state)?;
        let token = jfauth
            .token
            .as_deref()
            .context_unauthorized("forbidden", "forbidden")?;
        let device_id = jfauth
            .device_id
            .as_deref()
            .context_unauthorized("forbidden", "forbidden")?;

        let mut conn = state.db.get_conn()?;
        let device = Device::get_by_access_token(&mut conn, token)
            .map_err(ApiError::internal_server_error)?
            .context_unauthorized("forbidden", "forbidden")?;
        let user = crate::db::User::get_by_id(&mut conn, &device.user_id)
            .map_err(ApiError::internal_server_error)?
            .context_unauthorized("forbidden", "forbidden")?;

        Ok(Self {
            device,
            user,
            aio: state.aio.clone(),
        })
    }
}

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
            None => return Ok(Self::default()),
        };

        if !scheme.eq_ignore_ascii_case("MediaBrowser")
            && !scheme.eq_ignore_ascii_case("Emby")
        {
            return Ok(Self::default());
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
        })
    }
}

impl FromRequestParts<crate::AppState> for JellyfinAuthHeader {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &crate::AppState,
    ) -> ApiResult<Self> {
        let raw = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                parts
                    .headers
                    .get("X-Emby-Authorization")
                    .and_then(|v| v.to_str().ok())
            })
            .context_unauthorized("forbidden", "forbidden")?;

        Ok(Self::from_str(raw)?)
    }
}
