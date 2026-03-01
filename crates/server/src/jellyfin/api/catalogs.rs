use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use remux_macros::{delete, get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CatalogDto {
    pub id: Uuid,
    pub title: String,
    pub promoted: bool,
    pub catalog_media_kind: String,
}

impl From<db::Media> for CatalogDto {
    fn from(m: db::Media) -> Self {
        let promoted = m.is_promoted();
        let catalog_media_kind = m
            .catalog_media_kind
            .map(|k| k.to_string())
            .unwrap_or_default();
        Self {
            id: m.id,
            title: m.title,
            promoted,
            catalog_media_kind,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CatalogRequest {
    pub title: String,
    pub promoted: bool,
    /// "movie" or "series"
    pub catalog_media_kind: String,
}

fn parse_media_kind(s: &str) -> Result<db::MediaKind> {
    db::MediaKind::try_from(s)
        .map_err(|_| anyhow::anyhow!("invalid catalog_media_kind: {s}"))
        .context_bad_request("Bad Request", "catalog_media_kind must be 'movie' or 'series'")
}

/// List all catalogs.
#[get("/admin/catalogs")]
pub async fn list_catalogs(
    State(state): State<AppState>,
    session: auth::AdminSession,
) -> Result<Json<Vec<CatalogDto>>> {
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(result.records.into_iter().map(CatalogDto::from).collect()))
}

/// Create a catalog.
#[post("/admin/catalogs")]
pub async fn create_catalog(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<CatalogRequest>,
) -> Result<Json<CatalogDto>> {
    let catalog_media_kind = parse_media_kind(&payload.catalog_media_kind)?;

    let mut media = db::Media {
        title: payload.title,
        kind: db::MediaKind::Catalog,
        catalog_kind: Some(db::CatalogKind::Smart),
        catalog_media_kind: Some(catalog_media_kind),
        promoted: if payload.promoted { 1 } else { 0 },
        ..Default::default()
    };

    media.save(&state.ctx.db).await?;

    Ok(Json(CatalogDto::from(media)))
}

/// Update a catalog.
#[post("/admin/catalogs/{id}")]
pub async fn update_catalog(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<CatalogRequest>,
) -> Result<Json<CatalogDto>> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Catalog not found")?;

    if media.kind != db::MediaKind::Catalog {
        return Err(anyhow::anyhow!("not a catalog")).context_bad_request("Bad Request", "Item is not a catalog");
    }

    let catalog_media_kind = parse_media_kind(&payload.catalog_media_kind)?;
    let promoted: i64 = if payload.promoted { 1 } else { 0 };
    let updated_at = Utc::now().naive_utc();

    sqlx::query(
        "UPDATE media SET title = $1, promoted = $2, catalog_media_kind = $3, updated_at = $4 WHERE id = $5",
    )
    .bind(&payload.title)
    .bind(promoted)
    .bind(catalog_media_kind.to_string())
    .bind(updated_at)
    .bind(id)
    .execute(&state.ctx.db)
    .await?;

    let updated = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Catalog not found after update")?;

    Ok(Json(CatalogDto::from(updated)))
}

/// Delete a catalog.
#[delete("/admin/catalogs/{id}")]
pub async fn delete_catalog(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Catalog not found")?;

    if media.kind != db::MediaKind::Catalog {
        return Err(anyhow::anyhow!("not a catalog")).context_bad_request("Bad Request", "Item is not a catalog");
    }

    db::Media::delete(&state.ctx.db, &id).await?;

    Ok(StatusCode::NO_CONTENT)
}
