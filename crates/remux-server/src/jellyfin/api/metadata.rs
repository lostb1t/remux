use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt};

// ── Studios ───────────────────────────────────────────────────────────────────

#[get("/studios")]
pub async fn studios(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let records = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Studio]),
            limit: q.limit,
            ..Default::default()
        },
    )
    .await?
    .records;
    let total = records.len() as i64;
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: records
            .into_iter()
            .map(jellyfin::db_media_to_item)
            .collect(),
        total_record_count: total,
        start_index: q.start_index.unwrap_or(0),
    }))
}

#[get("/studios/{name}")]
pub async fn studio_by_name(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let record = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Studio]),
            title_contains: Some(name.clone()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .context_not_found("Not Found", "Studio not found")?;
    Ok(Json(jellyfin::db_media_to_item(record)))
}

// ── Years ─────────────────────────────────────────────────────────────────────

#[get("/years")]
pub async fn years(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_else(|| {
            vec![jellyfin::MediaType::Movie, jellyfin::MediaType::Series]
        })
        .into_iter()
        .filter_map(|t| db::MediaKind::try_from(t).ok())
        .collect();

    let year_vals = db::Media::get_distinct_years(&state.ctx.db, &kinds).await?;
    let items: Vec<jellyfin::BaseItemDto> = year_vals
        .into_iter()
        .map(|y| {
            let id = Uuid::new_v5(&Uuid::NAMESPACE_OID, y.to_string().as_bytes());
            jellyfin::BaseItemDto {
                id,
                name: Some(y.to_string()),
                type_: jellyfin::MediaType::Year,
                production_year: Some(y),
                ..Default::default()
            }
        })
        .collect();

    let total = items.len() as i64;
    let start = q.start_index.unwrap_or(0);
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: start,
    }))
}

#[get("/years/{year}")]
pub async fn year_by_value(
    _state: State<AppState>,
    _session: auth::AuthSession,
    Path(year): Path<i64>,
) -> Result<impl IntoResponse> {
    let id = Uuid::new_v5(&Uuid::NAMESPACE_OID, year.to_string().as_bytes());
    Ok(Json(jellyfin::BaseItemDto {
        id,
        name: Some(year.to_string()),
        type_: jellyfin::MediaType::Year,
        production_year: Some(year),
        ..Default::default()
    }))
}

// ── Persons ───────────────────────────────────────────────────────────────────

#[get("/persons/{name}")]
pub async fn person_by_name(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let record = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Person]),
            title_contains: Some(name.clone()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .context_not_found("Not Found", "Person not found")?;
    Ok(Json(jellyfin::db_media_to_item(record)))
}

// ── Genres ────────────────────────────────────────────────────────────────────

#[get("/genres/{name}")]
pub async fn genre_by_name(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let record = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Genre]),
            title_contains: Some(name.clone()),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .context_not_found("Not Found", "Genre not found")?;
    Ok(Json(jellyfin::db_media_to_item(record)))
}
