use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{delete, get, post};
use serde::Deserialize;

use crate::AppState;
use crate::db::ApiKey;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, IntoApiError};

#[derive(Deserialize)]
pub struct CreateKeyQuery {
    pub app: String,
}

/// List all API keys (admin only)
#[get("/auth/keys")]
pub async fn get_api_keys(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(
            anyhow::anyhow!("").context_forbidden("Forbidden", "Admin access required")
        );
    }
    let keys = ApiKey::get_all(&state.ctx.db).await?;
    let items: Vec<jellyfin::AuthenticationInfo> = keys
        .into_iter()
        .map(|k| jellyfin::AuthenticationInfo {
            access_token: Some(k.access_token),
            app_name: Some(k.app_name),
            date_created: Some(k.created_at),
            is_active: Some(true),
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(jellyfin::QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
        ..Default::default()
    }))
}

/// Create a new API key (admin only)
#[post("/auth/keys")]
pub async fn create_api_key(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(params): Query<CreateKeyQuery>,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(
            anyhow::anyhow!("").context_forbidden("Forbidden", "Admin access required")
        );
    }
    ApiKey::create(&state.ctx.db, &params.app).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Delete / revoke an API key (admin only)
#[delete("/auth/keys/{key}")]
pub async fn delete_api_key(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(key): Path<String>,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(
            anyhow::anyhow!("").context_forbidden("Forbidden", "Admin access required")
        );
    }
    ApiKey::delete(&state.ctx.db, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
