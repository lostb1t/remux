use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, patch, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth::{AdminSession, AuthSession};
use crate::api;

// --------------------------------------------------------------------------
// GET /livetv/info
// --------------------------------------------------------------------------

#[get("/livetv/info")]
pub async fn livetv_info(
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    let channel_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvChannel]),
        ..Default::default()
    };
    let user_filter = db::UserFilter::default();
    let (channel_result, users) = tokio::join!(
        db::Media::get_by_filter(&state.ctx.db, &channel_filter),
        db::User::get_by_filter(&state.ctx.db, &user_filter),
    );
    let has_channels = !channel_result?.records.is_empty();
    let user_ids: Vec<String> = users?
        .records
        .into_iter()
        .map(|u| u.id.to_string())
        .collect();

    Ok(Json(serde_json::json!({
        "IsEnabled": has_channels,
        "EnabledUsers": user_ids,
    })))
}

// --------------------------------------------------------------------------
// GET /livetv/channels
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetChannelsQuery {
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
}

#[get("/livetv/channels")]
pub async fn livetv_channels(
    State(state): State<AppState>,
    _session: AuthSession,
    Query(q): Query<GetChannelsQuery>,
) -> Result<impl IntoResponse> {
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvChannel]),
            enabled: Some(true),
            limit: q.limit,
            offset: q.start_index,
            total_count: true,
            ..Default::default()
        },
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(api::db_media_to_item)
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: q.start_index.unwrap_or(0) as i32,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/channels/{channelId}
// --------------------------------------------------------------------------

#[get("/livetv/channels/{channel_id}")]
pub async fn livetv_channel(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(channel_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &channel_id)
        .await?
        .context_not_found("not found", "channel not found")?;
    Ok(Json(api::db_media_to_item(media)))
}

// --------------------------------------------------------------------------
// GET /livetv/programs/recommended
// --------------------------------------------------------------------------

#[get("/livetv/programs/recommended")]
pub async fn livetv_programs_recommended(
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/programs
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetProgramsQuery {
    pub channel_ids: Option<String>,
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
}

#[get("/livetv/programs")]
pub async fn livetv_programs(
    State(state): State<AppState>,
    _session: AuthSession,
    Query(q): Query<GetProgramsQuery>,
) -> Result<impl IntoResponse> {
    let parent_ids: Vec<Uuid> = q
        .channel_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse::<Uuid>().ok())
        .collect();

    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: q.limit,
        offset: q.start_index,
        total_count: true,
        ..Default::default()
    };

    if parent_ids.len() == 1 {
        filter.parent_id = Some(parent_ids[0]);
    }

    let result = db::Media::get_by_filter(&state.ctx.db, &filter).await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(api::db_media_to_item)
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: q.start_index.unwrap_or(0) as i32,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// POST /livetv/programs
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetProgramsBody {
    pub channel_ids: Option<Vec<Uuid>>,
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
}

#[post("/livetv/programs")]
pub async fn livetv_programs_post(
    State(state): State<AppState>,
    _session: AuthSession,
    Json(body): Json<GetProgramsBody>,
) -> Result<impl IntoResponse> {
    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: body.limit,
        offset: body.start_index,
        total_count: true,
        ..Default::default()
    };

    if let Some(ids) = body.channel_ids {
        if ids.len() == 1 {
            filter.parent_id = Some(ids[0]);
        }
    }

    let result = db::Media::get_by_filter(&state.ctx.db, &filter).await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(api::db_media_to_item)
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: body.start_index.unwrap_or(0) as i32,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/programs/{programId}
// --------------------------------------------------------------------------

#[get("/livetv/programs/{program_id}")]
pub async fn livetv_program(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(program_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &program_id)
        .await?
        .context_not_found("not found", "program not found")?;
    Ok(Json(api::db_media_to_item(media)))
}

// --------------------------------------------------------------------------
// GET /livetv/tunerhosts
// --------------------------------------------------------------------------

#[get("/livetv/tunerhosts")]
pub async fn livetv_tuner_hosts(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<impl IntoResponse> {
    let sources = db::IptvSource::get_all(&state.ctx.db).await?;
    let hosts: Vec<_> = sources.iter().map(iptv_source_to_tuner_host).collect();
    Ok(Json(hosts))
}

// --------------------------------------------------------------------------
// POST /livetv/tunerhosts  (create or update by Id)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TunerHostInfo {
    pub id: Option<String>,
    #[serde(rename = "FriendlyName")]
    pub friendly_name: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[post("/livetv/tunerhosts")]
pub async fn livetv_add_tuner_host(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(body): Json<TunerHostInfo>,
) -> Result<impl IntoResponse> {
    let source_type = body.type_.as_deref().unwrap_or("m3u").to_string();
    let url = body
        .url
        .context_bad_request("bad request", "url is required")?;

    // If Id is present and parses as UUID, update existing; otherwise create new.
    let id = body
        .id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(crate::utils::get_uuid);

    // For Xtream updates, preserve existing password if new one is blank.
    let password = if source_type == "xtream" {
        let provided = body.password.filter(|p| !p.is_empty());
        if provided.is_none() {
            // Fetch existing to keep password
            db::IptvSource::get_by_id(&state.ctx.db, &id)
                .await?
                .and_then(|s| s.xtream_password)
        } else {
            provided
        }
    } else {
        None
    };

    let source = db::IptvSource {
        id,
        name: body.friendly_name.unwrap_or_else(|| "IPTV".to_string()),
        m3u_url: url,
        source_type,
        xtream_username: body.username,
        xtream_password: password,
        ..Default::default()
    };
    source.save(&state.ctx.db).await?;
    Ok((StatusCode::OK, Json(iptv_source_to_tuner_host(&source))))
}

// --------------------------------------------------------------------------
// DELETE /livetv/tunerhosts  (?id=...)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeleteTunerQuery {
    pub id: Uuid,
}

#[delete("/livetv/tunerhosts")]
pub async fn livetv_delete_tuner_host(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<DeleteTunerQuery>,
) -> Result<impl IntoResponse> {
    let source_aio_id = q.id.simple().to_string();
    sqlx::query("DELETE FROM media WHERE media_id = $1 AND kind = 'tv_channel'")
        .bind(&source_aio_id)
        .execute(&state.ctx.db)
        .await?;

    db::IptvSource::delete(&state.ctx.db, &q.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// GET /livetv/tunerhosts/default
// --------------------------------------------------------------------------

#[get("/livetv/tunerhosts/default")]
pub async fn livetv_tuner_host_default(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse> {
    let ty = q.get("type").cloned().unwrap_or_else(|| "m3u".to_string());
    Ok(Json(serde_json::json!({
        "Id": "",
        "Url": "",
        "FriendlyName": "",
        "Type": ty,
        "Status": "Online",
        "RefreshKey": "",
    })))
}

// --------------------------------------------------------------------------
// GET /remux/iptv/epgsources
// --------------------------------------------------------------------------

#[get("/remux/iptv/epgsources")]
pub async fn remux_epg_sources(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<impl IntoResponse> {
    let sources = db::EpgSource::get_all(&state.ctx.db).await?;
    Ok(Json(
        sources.iter().map(epg_source_to_dto).collect::<Vec<_>>(),
    ))
}

// --------------------------------------------------------------------------
// POST /remux/iptv/epgsources  (create or update by Id)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct EpgSourcePayload {
    pub id: Option<String>,
    pub name: String,
    pub url: String,
}

#[post("/remux/iptv/epgsources")]
pub async fn remux_save_epg_source(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(body): Json<EpgSourcePayload>,
) -> Result<impl IntoResponse> {
    let id = body
        .id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(crate::utils::get_uuid);

    let source = db::EpgSource {
        id,
        name: body.name,
        url: body.url,
        ..Default::default()
    };
    source.save(&state.ctx.db).await?;
    Ok((StatusCode::OK, Json(epg_source_to_dto(&source))))
}

// --------------------------------------------------------------------------
// DELETE /remux/iptv/epgsources  (?id=...)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeleteEpgQuery {
    pub id: Uuid,
}

#[delete("/remux/iptv/epgsources")]
pub async fn remux_delete_epg_source(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<DeleteEpgQuery>,
) -> Result<impl IntoResponse> {
    db::EpgSource::delete(&state.ctx.db, &q.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// GET /remux/iptv/channels  (all channels, including disabled)
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct GetAllChannelsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
}

#[get("/remux/iptv/channels")]
pub async fn remux_iptv_channels(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<GetAllChannelsQuery>,
) -> Result<impl IntoResponse> {
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvChannel]),
            limit: q.limit,
            offset: q.offset,
            title_contains: q.search.filter(|s| !s.is_empty()),
            total_count: true,
            ..Default::default()
        },
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(|m| channel_to_editor_dto(&m))
        .collect();

    Ok(Json(serde_json::json!({
        "Items": dtos,
        "TotalRecordCount": result.total_count,
    })))
}

// --------------------------------------------------------------------------
// POST /remux/iptv/channels/bulk  (set enabled for all / search results)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BulkChannelBody {
    pub enabled: bool,
    pub search: Option<String>,
}

#[post("/remux/iptv/channels/bulk")]
pub async fn remux_bulk_channels(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(body): Json<BulkChannelBody>,
) -> Result<impl IntoResponse> {
    let enabled_val = if body.enabled { 1i64 } else { 0i64 };
    if let Some(search) = body.search.filter(|s| !s.is_empty()) {
        sqlx::query(
            "UPDATE media SET enabled = $1, updated_at = datetime('now')
             WHERE kind = 'tv_channel' AND (title LIKE $2 OR custom_name LIKE $2)",
        )
        .bind(enabled_val)
        .bind(format!("%{search}%"))
        .execute(&state.ctx.db)
        .await?;
    } else {
        sqlx::query(
            "UPDATE media SET enabled = $1, updated_at = datetime('now') WHERE kind = 'tv_channel'",
        )
        .bind(enabled_val)
        .execute(&state.ctx.db)
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// PATCH /remux/iptv/channels/{id}
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PatchChannelBody {
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
    pub custom_name: Option<String>,
}

#[patch("/remux/iptv/channels/{id}")]
pub async fn remux_patch_channel(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchChannelBody>,
) -> Result<impl IntoResponse> {
    // Build a targeted UPDATE — only touch what was provided.
    // We always update updated_at.
    sqlx::query(
        r#"
        UPDATE media SET
            enabled    = COALESCE($1, enabled),
            sort_order = COALESCE($2, sort_order),
            custom_name = $3,
            updated_at = datetime('now')
        WHERE id = $4 AND kind = 'tv_channel'
        "#,
    )
    .bind(body.enabled.map(|b| if b { 1i64 } else { 0i64 }))
    .bind(body.sort_order)
    .bind(body.custom_name)
    .bind(id)
    .execute(&state.ctx.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// Conversion helpers
// --------------------------------------------------------------------------

fn iptv_source_to_tuner_host(source: &db::IptvSource) -> serde_json::Value {
    serde_json::json!({
        "Id": source.id.simple().to_string(),
        "Url": source.m3u_url,
        "FriendlyName": source.name,
        "Type": source.source_type,
        "Username": source.xtream_username,
        "Status": "Online",
    })
}

#[derive(Debug, Serialize)]
pub struct EpgSourceDto {
    pub id: String,
    pub name: String,
    pub url: String,
}

fn epg_source_to_dto(source: &db::EpgSource) -> EpgSourceDto {
    EpgSourceDto {
        id: source.id.simple().to_string(),
        name: source.name.clone(),
        url: source.url.clone(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChannelEditorDto {
    pub id: String,
    pub name: String,
    pub custom_name: Option<String>,
    pub channel_number: Option<i64>,
    pub sort_order: Option<i64>,
    pub enabled: bool,
    pub logo: Option<String>,
    pub group: Option<String>,
}

fn channel_to_editor_dto(m: &db::Media) -> ChannelEditorDto {
    ChannelEditorDto {
        id: m.id.simple().to_string(),
        name: m.title.clone(),
        custom_name: m.custom_name.clone(),
        channel_number: m.channel_number,
        sort_order: m.sort_order,
        enabled: m.enabled != 0,
        logo: m.poster.clone(),
        group: m.media_id.clone(),
    }
}
