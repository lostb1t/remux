use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{api_query, delete, get, post};

use crate::{
    AppState, api,
    db::{ApiKey, auth},
};
use axum_anyhow::{ApiResult as Result, IntoApiError};

#[api_query]
pub struct CreateKeyQuery {
    pub app: String,
}

/// List all API keys (admin only)
#[get("/auth/keys")]
pub async fn get_api_keys(
    State(state): State<AppState>,
    session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let keys = ApiKey::get_all(
        &state
            .ctx
            .db,
    )
    .await?;
    let items: Vec<api::AuthenticationInfo> = keys
        .into_iter()
        .map(|k| api::AuthenticationInfo {
            access_token: Some(k.access_token),
            app_name: Some(k.app_name),
            date_created: Some(k.created_at),
            is_active: Some(true),
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(api::QueryResult {
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
    session: auth::AdminSession,
    Query(params): Query<CreateKeyQuery>,
) -> Result<impl IntoResponse> {
    let key = ApiKey::create(
        &state
            .ctx
            .db,
        &params.app,
    )
    .await?;
    let info = api::AuthenticationInfo {
        access_token: Some(key.access_token),
        app_name: Some(key.app_name),
        date_created: Some(key.created_at),
        is_active: Some(true),
    };
    Ok((StatusCode::OK, Json(info)))
}

/// Delete / revoke an API key (admin only)
#[delete("/auth/keys/{key}")]
pub async fn delete_api_key(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(key): Path<String>,
) -> Result<impl IntoResponse> {
    ApiKey::delete(
        &state
            .ctx
            .db,
        &key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
