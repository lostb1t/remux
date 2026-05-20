use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};
use axum_extra::extract::Query;
use chrono::{Duration, Utc};
use http::StatusCode;
use remux_macros::{delete, get, patch, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::IptvSourceType;
use crate::db::auth::{AdminSession, AuthSession};

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
// GET /livetv/guideinfo
// --------------------------------------------------------------------------

#[get("/livetv/guideinfo")]
pub async fn livetv_guide_info(
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    use sqlx::Row as _;
    let row = sqlx::query(
        "SELECT MIN(live_start), MAX(live_end) FROM media WHERE kind = 'tv_program'",
    )
    .fetch_one(&state.ctx.db)
    .await?;

    let now = Utc::now().naive_utc();
    let start = row
        .get::<Option<String>, _>(0)
        .unwrap_or_else(|| now.to_string());
    let end = row
        .get::<Option<String>, _>(1)
        .unwrap_or_else(|| (now + Duration::days(14)).to_string());

    let fmt = |s: String| s.replace(' ', "T") + "Z";
    Ok(Json(serde_json::json!({
        "StartDate": fmt(start),
        "EndDate":   fmt(end),
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

#[derive(Debug, Default, Deserialize)]
pub struct GetRecommendedQuery {
    #[serde(rename = "limit", alias = "Limit")]
    pub limit: Option<u32>,
}

#[get("/livetv/programs/recommended")]
pub async fn livetv_programs_recommended(
    State(state): State<AppState>,
    _session: AuthSession,
    Query(q): Query<GetRecommendedQuery>,
) -> Result<impl IntoResponse> {
    let now = Utc::now().naive_utc();
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvProgram]),
            parent_enabled: Some(true),
            min_end_date: Some(now),
            max_start_date: Some(now),
            sort_by_channel_order: true,
            limit: Some(q.limit.unwrap_or(20)),
            total_count: false,
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
        total_record_count: dtos.len() as i64,
        start_index: 0,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/programs
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct GetProgramsQuery {
    /// Accepts both repeated params (`channelIds=a&channelIds=b`) and a
    /// single comma-separated value (`channelIds=a,b`) for client compat.
    #[serde(rename = "channelIds", alias = "ChannelIds", default)]
    pub channel_ids_raw: Option<String>,
    #[serde(rename = "startIndex", alias = "StartIndex")]
    pub start_index: Option<u32>,
    // Jellyfin sends lowercase "limit" on this endpoint (unlike most others)
    #[serde(rename = "limit", alias = "Limit")]
    pub limit: Option<u32>,
    #[serde(rename = "HasAired")]
    pub has_aired: Option<bool>,
    #[serde(rename = "EnableTotalRecordCount")]
    pub enable_total_record_count: Option<bool>,
    #[serde(rename = "minEndDate", alias = "MinEndDate")]
    pub min_end_date: Option<String>,
    #[serde(rename = "maxStartDate", alias = "MaxStartDate")]
    pub max_start_date: Option<String>,
    #[serde(rename = "isMovie", alias = "IsMovie")]
    pub is_movie: Option<bool>,
    #[serde(rename = "isSeries", alias = "IsSeries")]
    pub is_series: Option<bool>,
    #[serde(rename = "isNews", alias = "IsNews")]
    pub is_news: Option<bool>,
    #[serde(rename = "isKids", alias = "IsKids")]
    pub is_kids: Option<bool>,
    #[serde(rename = "isSports", alias = "IsSports")]
    pub is_sports: Option<bool>,
}

#[get("/livetv/programs")]
pub async fn livetv_programs(
    State(state): State<AppState>,
    _session: AuthSession,
    Query(q): Query<GetProgramsQuery>,
) -> Result<impl IntoResponse> {
    let channel_ids: Vec<Uuid> = q
        .channel_ids_raw
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();

    let parse_dt = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.naive_utc())
    };

    let mut program_kinds = vec![];
    if q.is_movie == Some(true) {
        program_kinds.push(db::ProgramKind::Movie);
    }
    if q.is_series == Some(true) {
        program_kinds.push(db::ProgramKind::Series);
    }
    if q.is_news == Some(true) {
        program_kinds.push(db::ProgramKind::News);
    }
    if q.is_kids == Some(true) {
        program_kinds.push(db::ProgramKind::Kids);
    }
    if q.is_sports == Some(true) {
        program_kinds.push(db::ProgramKind::Sports);
    }

    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: Some(q.limit.unwrap_or(500)),
        offset: q.start_index,
        total_count: q.enable_total_record_count.unwrap_or(true),
        has_aired: q.has_aired,
        parent_enabled: Some(true),
        min_end_date: q.min_end_date.as_deref().and_then(parse_dt),
        max_start_date: q.max_start_date.as_deref().and_then(parse_dt),
        program_kinds: if program_kinds.is_empty() {
            None
        } else {
            Some(program_kinds)
        },
        ..Default::default()
    };

    match channel_ids.len() {
        1 => filter.parent_id = Some(channel_ids[0]),
        n if n > 1 => filter.parent_ids = Some(channel_ids),
        _ => {}
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
    pub has_aired: Option<bool>,
    pub enable_total_record_count: Option<bool>,
    pub min_end_date: Option<String>,
    pub max_start_date: Option<String>,
}

#[post("/livetv/programs")]
pub async fn livetv_programs_post(
    State(state): State<AppState>,
    _session: AuthSession,
    Json(body): Json<GetProgramsBody>,
) -> Result<impl IntoResponse> {
    let parse_dt = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.naive_utc())
    };

    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: Some(body.limit.unwrap_or(500)),
        offset: body.start_index,
        total_count: body.enable_total_record_count.unwrap_or(true),
        has_aired: body.has_aired,
        parent_enabled: Some(true),
        min_end_date: body.min_end_date.as_deref().and_then(parse_dt),
        max_start_date: body.max_start_date.as_deref().and_then(parse_dt),
        ..Default::default()
    };

    if let Some(ids) = body.channel_ids {
        match ids.len() {
            1 => filter.parent_id = Some(ids[0]),
            n if n > 1 => filter.parent_ids = Some(ids),
            _ => {}
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
// GET /livetv/seriestimers
// --------------------------------------------------------------------------

#[get("/livetv/seriestimers")]
pub async fn livetv_series_timers(_session: AuthSession) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/timers
// --------------------------------------------------------------------------

#[get("/livetv/timers")]
pub async fn livetv_timers(_session: AuthSession) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/recordings/folders
// --------------------------------------------------------------------------

#[get("/livetv/recordings/folders")]
pub async fn livetv_recording_folders(
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/recordings
// --------------------------------------------------------------------------

#[get("/livetv/recordings")]
pub async fn livetv_recordings(_session: AuthSession) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/recordings/groups
// --------------------------------------------------------------------------

#[get("/livetv/recordings/groups")]
pub async fn livetv_recording_groups(
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/recordings/groups/{groupId}
// --------------------------------------------------------------------------

#[get("/livetv/recordings/groups/{group_id}")]
pub async fn livetv_recording_group(
    _session: AuthSession,
    Path(_group_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_FOUND)
}

// --------------------------------------------------------------------------
// GET /livetv/recordings/series
// --------------------------------------------------------------------------

#[get("/livetv/recordings/series")]
pub async fn livetv_recordings_series(
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::QueryResult::<api::BaseItemDto> {
        total_record_count: 0,
        start_index: 0,
        items: vec![],
    }))
}

// --------------------------------------------------------------------------
// GET + DELETE /livetv/recordings/{recordingId}
// --------------------------------------------------------------------------

#[get("/livetv/recordings/{recording_id}")]
pub async fn livetv_recording(
    _session: AuthSession,
    Path(_recording_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_FOUND)
}

#[delete("/livetv/recordings/{recording_id}")]
pub async fn livetv_delete_recording(
    _session: AuthSession,
    Path(_recording_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_FOUND)
}

// --------------------------------------------------------------------------
// GET /livetv/liverecordings/{recordingId}/stream
// --------------------------------------------------------------------------

#[get("/livetv/liverecordings/{recording_id}/stream")]
pub async fn livetv_live_recording_stream(
    _session: AuthSession,
    Path(_recording_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_FOUND)
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
    let source_type = body
        .type_
        .as_deref()
        .and_then(|s| s.parse::<IptvSourceType>().ok())
        .unwrap_or(IptvSourceType::M3u);
    let url = body
        .url
        .context_bad_request("bad request", "url is required")?;

    // If Id is present and parses as UUID, update existing; otherwise create new.
    let id = body
        .id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(crate::common::get_uuid);

    // For Xtream updates, preserve existing password if new one is blank.
    let password = if source_type == IptvSourceType::Xtream {
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
    let source_id = q.id.simple().to_string();
    sqlx::query(
        "DELETE FROM media
         WHERE kind = 'tv_program'
           AND parent_id IN (
               SELECT id FROM media
               WHERE kind = 'tv_channel' AND JSON_EXTRACT(external_ids, '$.iptv_source_id') = $1
           )",
    )
    .bind(&source_id)
    .execute(&state.ctx.db)
    .await?;

    sqlx::query("DELETE FROM media WHERE kind = 'tv_channel' AND JSON_EXTRACT(external_ids, '$.iptv_source_id') = $1")
        .bind(&source_id)
        .execute(&state.ctx.db)
        .await?;

    db::IptvSource::delete(&state.ctx.db, &q.id).await?;
    state.tasks.run_task("RefreshIptv").await?;
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
        .unwrap_or_else(crate::common::get_uuid);

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
    pub enabled: Option<bool>,
    pub country: Option<String>,
    pub sort: Option<String>,
}

#[get("/remux/iptv/channels")]
pub async fn remux_iptv_channels(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<GetAllChannelsQuery>,
) -> Result<impl IntoResponse> {
    let sort_by = match q.sort.as_deref() {
        Some("name") => vec![api::ItemSortBy::SortName],
        _ => vec![api::ItemSortBy::ChannelOrder],
    };
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvChannel]),
            limit: q.limit,
            offset: q.offset,
            title_contains: q.search.filter(|s| !s.is_empty()),
            enabled: q.enabled,
            country_filter: q.country.filter(|s| !s.is_empty()),
            sort_by,
            sort_order: vec![api::SortOrder::Ascending],
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
// GET /remux/iptv/channels/countries  (distinct country codes for TvChannels)
// --------------------------------------------------------------------------

#[get("/remux/iptv/channels/countries")]
pub async fn remux_iptv_channel_countries(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<impl IntoResponse> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT country FROM media \
         WHERE kind = 'tv_channel' AND country IS NOT NULL AND country != '' \
         ORDER BY country",
    )
    .fetch_all(&state.ctx.db)
    .await?;
    Ok(Json(rows))
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
    let enabled_val = body.enabled;
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
    .bind(body.enabled)
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
        "Type": source.source_type.to_string(),
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
    pub country: Option<String>,
}

fn channel_to_editor_dto(m: &db::Media) -> ChannelEditorDto {
    ChannelEditorDto {
        id: m.id.simple().to_string(),
        name: m.title.clone(),
        custom_name: m.custom_name.clone(),
        channel_number: m.channel_number,
        sort_order: m.sort_order,
        enabled: m.enabled,
        logo: m.images.get_path(db::ImageKind::Primary).map(str::to_owned),
        group: m.external_ids.iptv_source_id.clone(),
        country: m.country.clone(),
    }
}
