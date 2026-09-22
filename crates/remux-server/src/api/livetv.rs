use crate::{OptionExt, ResultExt};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use chrono::{Duration, Utc};
use http::StatusCode;
use remux_macros::{delete, get, patch, post, query};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    addons::dispatcharr_dvr::DispatcharrRecordingStatus,
    api, db,
    db::auth::{AdminSession, AuthSession},
    services::{
        DvrService,
        dvr_service::{CreateSeriesTimerRequest, CreateTimerRequest},
    },
    stream::{HttpSource, StreamSource},
};

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
        db::Media::get_by_filter(
            &state
                .ctx
                .db,
            &channel_filter
        ),
        db::User::get_by_filter(
            &state
                .ctx
                .db,
            &user_filter
        ),
    );
    let has_channels = !channel_result?
        .records
        .is_empty();
    let user_ids: Vec<String> = users?
        .records
        .into_iter()
        .map(|u| {
            u.id.to_string()
        })
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
    .fetch_one(
        &state
            .ctx
            .db,
    )
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

#[query]
#[derive(Debug, Default)]
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
    let mut result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
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

    db::Media::attach_streams(
        &state
            .ctx
            .db,
        &mut result.records,
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0) as i32,
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
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &channel_id,
    )
    .await?
    .context_not_found("channel not found")?;
    media.sources = Some(
        media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?,
    );
    Ok(Json(api::db_media_to_item(media, false)))
}

// --------------------------------------------------------------------------
// GET /livetv/programs/recommended
// --------------------------------------------------------------------------

#[query]
#[derive(Debug, Default)]
pub struct GetRecommendedQuery {
    #[serde(rename = "limit", alias = "Limit")]
    pub limit: Option<u32>,
}

#[get("/livetv/programs/recommended")]
pub async fn livetv_programs_recommended(
    State(state): State<AppState>,
    session: AuthSession,
    Query(q): Query<GetRecommendedQuery>,
) -> Result<impl IntoResponse> {
    let now = Utc::now().naive_utc();
    let policy = session
        .user
        .policy
        .as_ref();
    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvProgram]),
            parent_enabled: Some(true),
            min_end_date: Some(now),
            max_start_date: Some(now),
            sort_by_channel_order: true,
            limit: Some(
                q.limit
                    .unwrap_or(20),
            ),
            total_count: false,
            user_id: Some(
                session
                    .user
                    .id,
            ),
            max_parental_rating: policy.and_then(|p| p.max_parental_rating),
            blocked_tags: policy
                .map(|p| {
                    p.blocked_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            allowed_tags: policy
                .map(|p| {
                    p.allowed_tags
                        .clone()
                })
                .filter(|v| !v.is_empty()),
            policy_filter: policy
                .and_then(|p| {
                    p.filter_rules
                        .as_ref()
                })
                .cloned(),
            ..Default::default()
        },
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
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

#[query]
#[derive(Debug, Default)]
pub struct GetProgramsQuery {
    /// Accepts both repeated params (`channelIds=a&channelIds=b`) and a
    /// single comma-separated value (`channelIds=a,b`) for client compat.
    #[serde(rename = "channelIds", alias = "ChannelIds", default)]
    pub channel_ids: remux_sdks::CommaSeparatedList<Uuid>,
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
    #[serde(rename = "LibrarySeriesId")]
    pub library_series_id: Option<String>,
}

#[get("/livetv/programs")]
pub async fn livetv_programs(
    State(state): State<AppState>,
    session: AuthSession,
    Query(q): Query<GetProgramsQuery>,
) -> Result<impl IntoResponse> {
    if q.library_series_id
        .is_some()
    {
        return Ok(Json(api::QueryResult {
            total_record_count: 0,
            start_index: 0,
            items: vec![],
        }));
    }

    let channel_ids: Vec<Uuid> = q
        .channel_ids
        .to_vec();

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

    let policy = session
        .user
        .policy
        .as_ref();
    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: q.limit,
        offset: q.start_index,
        total_count: q
            .enable_total_record_count
            .unwrap_or(true),
        has_aired: q.has_aired,
        parent_enabled: Some(true),
        min_end_date: q
            .min_end_date
            .as_deref()
            .and_then(parse_dt),
        max_start_date: q
            .max_start_date
            .as_deref()
            .and_then(parse_dt),
        program_kinds: if program_kinds.is_empty() {
            None
        } else {
            Some(program_kinds)
        },
        user_id: Some(
            session
                .user
                .id,
        ),
        max_parental_rating: policy.and_then(|p| p.max_parental_rating),
        blocked_tags: policy
            .map(|p| {
                p.blocked_tags
                    .clone()
            })
            .filter(|v| !v.is_empty()),
        allowed_tags: policy
            .map(|p| {
                p.allowed_tags
                    .clone()
            })
            .filter(|v| !v.is_empty()),
        policy_filter: policy
            .and_then(|p| {
                p.filter_rules
                    .as_ref()
            })
            .cloned(),
        ..Default::default()
    };

    match channel_ids.len() {
        1 => filter.parent_id = Some(channel_ids[0]),
        n if n > 1 => filter.parent_ids = Some(channel_ids),
        _ => {}
    }

    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &filter,
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0) as i32,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// POST /livetv/programs
// --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetProgramsBody {
    #[serde(rename = "channelIds", alias = "ChannelIds", default)]
    pub channel_ids: remux_sdks::CommaSeparatedList<Uuid>,
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
    session: AuthSession,
    Json(body): Json<GetProgramsBody>,
) -> Result<impl IntoResponse> {
    let parse_dt = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.naive_utc())
    };

    let policy = session
        .user
        .policy
        .as_ref();
    let mut filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvProgram]),
        limit: body.limit,
        offset: body.start_index,
        total_count: body
            .enable_total_record_count
            .unwrap_or(true),
        has_aired: body.has_aired,
        parent_enabled: Some(true),
        min_end_date: body
            .min_end_date
            .as_deref()
            .and_then(parse_dt),
        max_start_date: body
            .max_start_date
            .as_deref()
            .and_then(parse_dt),
        user_id: Some(
            session
                .user
                .id,
        ),
        max_parental_rating: policy.and_then(|p| p.max_parental_rating),
        blocked_tags: policy
            .map(|p| {
                p.blocked_tags
                    .clone()
            })
            .filter(|v| !v.is_empty()),
        allowed_tags: policy
            .map(|p| {
                p.allowed_tags
                    .clone()
            })
            .filter(|v| !v.is_empty()),
        policy_filter: policy
            .and_then(|p| {
                p.filter_rules
                    .as_ref()
            })
            .cloned(),
        ..Default::default()
    };

    match body
        .channel_ids
        .len()
    {
        1 => filter.parent_id = Some(body.channel_ids[0]),
        n if n > 1 => {
            filter.parent_ids = Some(
                body.channel_ids
                    .to_vec(),
            )
        }
        _ => {}
    }

    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &filter,
    )
    .await?;

    let dtos: Vec<_> = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: result.total_count as i64,
        start_index: body
            .start_index
            .unwrap_or(0) as i32,
        items: dtos,
    }))
}

// --------------------------------------------------------------------------
// GET /livetv/seriestimers
// --------------------------------------------------------------------------

#[get("/livetv/seriestimers")]
pub async fn livetv_series_timers(
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    let items = DvrService::list_series_timers(&state.ctx).await?;
    Ok(Json(api::QueryResult {
        total_record_count: items.len() as i64,
        start_index: 0,
        items,
    }))
}

// --------------------------------------------------------------------------
// POST /livetv/seriestimers
// --------------------------------------------------------------------------

#[post("/livetv/seriestimers")]
pub async fn livetv_create_series_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Json(body): Json<CreateSeriesTimerRequest>,
) -> Result<impl IntoResponse> {
    let timer = DvrService::create_series_timer(&state.ctx, body).await?;
    Ok((StatusCode::OK, Json(timer)))
}

// --------------------------------------------------------------------------
// GET /livetv/seriestimers/{timerId}
// --------------------------------------------------------------------------

#[get("/livetv/seriestimers/{timer_id}")]
pub async fn livetv_get_series_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(timer_id): Path<String>,
) -> Result<impl IntoResponse> {
    let items = DvrService::list_series_timers(&state.ctx).await?;
    let found = items
        .into_iter()
        .find(|t| t.id == timer_id)
        .context_not_found("series timer not found")?;
    Ok(Json(found))
}

// --------------------------------------------------------------------------
// DELETE /livetv/seriestimers/{timerId}
// --------------------------------------------------------------------------

#[delete("/livetv/seriestimers/{timer_id}")]
pub async fn livetv_delete_series_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(timer_id): Path<String>,
) -> Result<impl IntoResponse> {
    if DvrService::delete_series_timer(&state.ctx, &timer_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

// --------------------------------------------------------------------------
// GET /livetv/timers
// --------------------------------------------------------------------------

#[get("/livetv/timers")]
pub async fn livetv_timers(
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    let items = DvrService::list_timers(&state.ctx).await?;
    Ok(Json(api::QueryResult {
        total_record_count: items.len() as i64,
        start_index: 0,
        items,
    }))
}

// --------------------------------------------------------------------------
// POST /livetv/timers
// --------------------------------------------------------------------------

#[post("/livetv/timers")]
pub async fn livetv_create_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Json(body): Json<CreateTimerRequest>,
) -> Result<impl IntoResponse> {
    let timer = DvrService::create_timer(&state.ctx, body).await?;
    Ok((StatusCode::OK, Json(timer)))
}

// --------------------------------------------------------------------------
// GET /livetv/timers/{timerId}
// --------------------------------------------------------------------------

#[get("/livetv/timers/{timer_id}")]
pub async fn livetv_get_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(timer_id): Path<i64>,
) -> Result<impl IntoResponse> {
    let timer = DvrService::get_timer(&state.ctx, timer_id)
        .await?
        .context_not_found("timer not found")?;
    Ok(Json(timer))
}

// --------------------------------------------------------------------------
// DELETE /livetv/timers/{timerId}
// --------------------------------------------------------------------------

#[delete("/livetv/timers/{timer_id}")]
pub async fn livetv_delete_timer(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(timer_id): Path<String>,
) -> Result<impl IntoResponse> {
    let Some(timer_id) = DvrService::resolve_timer_id(&state.ctx, &timer_id).await
    else {
        return Ok(StatusCode::NOT_FOUND);
    };
    if DvrService::delete_timer(&state.ctx, timer_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

// --------------------------------------------------------------------------
// GET /livetv/timers/defaults
// --------------------------------------------------------------------------

#[query]
pub struct TimerDefaultsQuery {
    #[serde(rename = "programId")]
    program_id: Option<Uuid>,
}

#[get("/livetv/timers/defaults")]
pub async fn livetv_timer_defaults(
    _session: AuthSession,
    Query(q): Query<TimerDefaultsQuery>,
) -> Result<impl IntoResponse> {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct TimerDefaults {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        program_id: Option<String>,
        pre_padding_seconds: i32,
        post_padding_seconds: i32,
        is_pre_padding_required: bool,
        is_post_padding_required: bool,
        priority: i32,
    }
    Ok(Json(TimerDefaults {
        id: String::new(),
        program_id: q
            .program_id
            .map(|id| id.to_string()),
        pre_padding_seconds: 0,
        post_padding_seconds: 0,
        is_pre_padding_required: false,
        is_post_padding_required: false,
        priority: 0,
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
pub async fn livetv_recordings(
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    let items = DvrService::list_recordings(&state.ctx).await?;
    Ok(Json(api::QueryResult {
        total_record_count: items.len() as i64,
        start_index: 0,
        items,
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
    State(state): State<AppState>,
    _session: AuthSession,
) -> Result<impl IntoResponse> {
    let recordings = DvrService::list_recordings(&state.ctx).await?;
    let mut seen = std::collections::HashSet::new();
    let items: Vec<api::BaseItemDto> = recordings
        .into_iter()
        .filter(|r| {
            r.name
                .as_ref()
                .is_some_and(|n| seen.insert(n.clone()))
        })
        .map(|r| api::BaseItemDto {
            id: Uuid::new_v5(&r.id, b"series"),
            name: r.name,
            type_: api::MediaType::Series,
            is_folder: true,
            ..Default::default()
        })
        .collect();
    Ok(Json(api::QueryResult {
        total_record_count: items.len() as i64,
        start_index: 0,
        items,
    }))
}

// --------------------------------------------------------------------------
// GET + DELETE /livetv/recordings/{recordingId}
// --------------------------------------------------------------------------

#[get("/livetv/recordings/{recording_id}")]
pub async fn livetv_recording(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(recording_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let item = DvrService::get_recording(&state.ctx, recording_id)
        .await?
        .context_not_found("recording not found")?;
    Ok(Json(item))
}

#[delete("/livetv/recordings/{recording_id}")]
pub async fn livetv_delete_recording(
    State(state): State<AppState>,
    _session: AuthSession,
    Path(recording_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if DvrService::delete_recording(&state.ctx, recording_id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

// --------------------------------------------------------------------------
// GET /livetv/liverecordings/{recordingId}/stream
//
// Unauthenticated by design, as `GET /stream/{id}` is: the internal ffprobe
// pass behind `/items/{id}/playbackinfo` fetches this URL with no session to
// attach.
// --------------------------------------------------------------------------

#[get("/livetv/liverecordings/{recording_id}/stream")]
pub async fn livetv_live_recording_stream(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    let Some((cfg, rec)) =
        DvrService::recording_playback_target(&state.ctx, recording_id).await?
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(file_url) = rec.file_url() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let url = format!("{}{file_url}", cfg.base_url);

    if file_url.contains("/hls/") || file_url.ends_with(".m3u8") {
        let resp = crate::addons::dispatcharr::CLIENT
            .get(&url)
            .header("X-API-Key", &cfg.api_key)
            .send()
            .await
            .context_bad_request("upstream HLS request failed")?
            .error_for_status()
            .context_bad_request("upstream HLS request failed")?
            .text()
            .await
            .context_bad_request("failed reading upstream playlist")?;
        // Only a recording still being written can grow.
        let growth = (rec.status() == DispatcharrRecordingStatus::Recording
            && rec.end_time > Utc::now())
        .then(|| Growth {
            scheduled_secs: (rec.end_time - rec.start_time).num_milliseconds() as f64
                / 1000.0,
            elapsed_secs: ((Utc::now() - rec.start_time).num_milliseconds() as f64
                / 1000.0)
                .max(0.0),
        });
        let rewritten = recording_vod_playlist(&resp, recording_id, growth);
        return Ok((
            [(http::header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
            rewritten,
        )
            .into_response());
    }

    let source = HttpSource {
        url,
        request_headers: std::collections::HashMap::from([(
            "X-API-Key".to_string(),
            cfg.api_key,
        )]),
        response_headers: Default::default(),
    };
    Ok(source
        .serve(&state, &headers)
        .await?
        .into_response())
}

/// How far a recording that is still being written is expected to grow.
struct Growth {
    /// Length between the recording's scheduled start and end.
    scheduled_secs: f64,
    /// Time since the scheduled start.
    elapsed_secs: f64,
}

/// Rewrites Dispatcharr's live playlist into a finite VOD one
/// (`PLAYLIST-TYPE:VOD` + `ENDLIST`), with segment URIs pointed at our segment
/// proxy so the client need not attach `X-API-Key`.
///
/// Given `growth`, it is padded to the recording's expected final length with
/// not-yet-written segments, continuing Dispatcharr's `seg_NNNNN.ts` numbering
/// at the mean duration of the real ones; the segment proxy holds a request
/// for one until it exists. Without `growth` it lists only what exists.
fn recording_vod_playlist(
    upstream: &str,
    recording_id: Uuid,
    growth: Option<Growth>,
) -> String {
    let route = |seg: &str| format!("/livetv/liverecordings/{recording_id}/hls/{seg}");
    let mut out: Vec<String> = Vec::new();
    let mut durations: Vec<f64> = Vec::new();
    let mut last_segment: Option<String> = None;
    let mut target_duration = 4.0_f64;

    for line in upstream
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        if line.starts_with("#EXT-X-PLAYLIST-TYPE")
            || line.starts_with("#EXT-X-ENDLIST")
        {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            if let Some(d) = rest
                .split(',')
                .next()
                .and_then(|d| {
                    d.trim()
                        .parse::<f64>()
                        .ok()
                })
            {
                durations.push(d);
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target_duration = rest
                .trim()
                .parse()
                .unwrap_or(target_duration);
        }
        if line.starts_with('#') {
            out.push(line.to_string());
        } else {
            let seg = line
                .rsplit('/')
                .next()
                .unwrap_or(line);
            out.push(route(seg));
            last_segment = Some(seg.to_string());
        }
    }
    out.insert(
        usize::from(
            out.first()
                .is_some_and(|l| l == "#EXTM3U"),
        ),
        "#EXT-X-PLAYLIST-TYPE:VOD".to_string(),
    );

    if let Some(growth) = growth {
        // `seg_00017.ts` -> ("seg_", 17, width 5, ".ts"). A name that doesn't
        // follow that shape can't be continued, so the playlist stays a snapshot.
        let (prefix, next, width, ext) = match &last_segment {
            None => ("seg_".to_string(), 0, 5, ".ts".to_string()),
            Some(seg) => {
                let Some((stem, ext)) = seg.rsplit_once('.') else {
                    return finish_playlist(out);
                };
                let digits = stem
                    .chars()
                    .rev()
                    .take_while(char::is_ascii_digit)
                    .count();
                let (prefix, number) = stem.split_at(stem.len() - digits);
                let Ok(n) = number.parse::<u64>() else {
                    return finish_playlist(out);
                };
                (prefix.to_string(), n + 1, digits, format!(".{ext}"))
            }
        };
        let mean = if durations.is_empty() {
            target_duration
        } else {
            durations
                .iter()
                .sum::<f64>()
                / durations.len() as f64
        };
        // Dispatcharr records a roughly constant lead ahead of the wall
        // clock, so it overruns its scheduled length by that much; carry the
        // lead measured so far or the tail is cut off.
        let recorded = durations
            .iter()
            .sum::<f64>();
        let lead = (recorded - growth.elapsed_secs).max(0.0);
        let remaining = growth.scheduled_secs + lead - recorded;
        if mean > 0.0 && remaining > 0.5 {
            let count = ((remaining / mean).ceil() as u64).min(MAX_PADDED_SEGMENTS);
            for k in 0..count {
                // The last one takes whatever is left, so the total matches.
                let d = if k + 1 == count {
                    (remaining - (count - 1) as f64 * mean).max(0.001)
                } else {
                    mean
                };
                out.push(format!("#EXTINF:{d:.6},"));
                out.push(route(&format!("{prefix}{:0width$}{ext}", next + k)));
            }
        }
    }
    finish_playlist(out)
}

fn finish_playlist(mut lines: Vec<String>) -> String {
    lines.push("#EXT-X-ENDLIST".to_string());
    let mut playlist = lines.join("\n");
    playlist.push('\n');
    playlist
}

/// Upper bound on how many not-yet-written segments a playlist is padded
/// with (~22 hours at 4 s), so a bogus schedule can't build a huge playlist.
const MAX_PADDED_SEGMENTS: u64 = 20_000;

/// How long a request for an unwritten segment is held, and its poll
/// interval. A client with a shorter HTTP timeout errors first.
const SEGMENT_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
const SEGMENT_POLL: std::time::Duration = std::time::Duration::from_millis(500);

// --------------------------------------------------------------------------
// GET /livetv/liverecordings/{recordingId}/hls/{segPath}
// --------------------------------------------------------------------------

#[get("/livetv/liverecordings/{recording_id}/hls/{seg_path}")]
pub async fn livetv_recording_hls_segment(
    State(state): State<AppState>,
    Path((recording_id, seg_path)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    let Some((cfg, rec)) =
        DvrService::recording_playback_target(&state.ctx, recording_id).await?
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let url = format!(
        "{}/api/channels/recordings/{}/hls/{seg_path}",
        cfg.base_url, rec.id
    );
    if rec.status() == DispatcharrRecordingStatus::Recording {
        wait_for_segment(&url, &cfg.api_key, SEGMENT_WAIT, SEGMENT_POLL, || async {
            matches!(
                DvrService::recording_playback_target(&state.ctx, recording_id).await,
                Ok(Some((_, rec))) if rec.status() == DispatcharrRecordingStatus::Recording
            )
        })
        .await;
    }
    let source = HttpSource {
        url,
        request_headers: std::collections::HashMap::from([(
            "X-API-Key".to_string(),
            cfg.api_key,
        )]),
        response_headers: Default::default(),
    };
    Ok(source
        .serve(&state, &headers)
        .await?
        .into_response())
}

/// Holds the request while Dispatcharr 404s a segment the padded playlist
/// lists but has not written yet. Returns once the segment exists,
/// `still_running` reports the recording has stopped, or `wait` elapses.
async fn wait_for_segment<F, Fut>(
    url: &str,
    api_key: &str,
    wait: std::time::Duration,
    poll: std::time::Duration,
    still_running: F,
) where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + wait;
    let mut polls = 0u32;
    loop {
        let missing = crate::addons::dispatcharr::CLIENT
            .head(url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .is_ok_and(|r| r.status() == StatusCode::NOT_FOUND);
        if !missing || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(poll).await;
        polls += 1;
        if polls % 10 == 0 && !still_running().await {
            return;
        }
    }
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
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &program_id,
    )
    .await?
    .context_not_found("program not found")?;
    let mut records = vec![media];
    db::Media::preload_parents(
        &state
            .ctx
            .db,
        &mut records,
    )
    .await;
    Ok(Json(api::db_media_to_item(records.remove(0), false)))
}

// --------------------------------------------------------------------------
// GET /livetv/tunerhosts  (stub — sources now managed as addons)
// --------------------------------------------------------------------------

#[get("/livetv/tunerhosts")]
pub async fn livetv_tuner_hosts(
    _state: State<AppState>,
    _session: AdminSession,
) -> Result<impl IntoResponse> {
    Ok(Json(serde_json::json!([])))
}

// --------------------------------------------------------------------------
// POST /livetv/tunerhosts  (stub)
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
    _state: State<AppState>,
    _session: AdminSession,
    _body: Json<TunerHostInfo>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_IMPLEMENTED)
}

// --------------------------------------------------------------------------
// DELETE /livetv/tunerhosts  (stub)
// --------------------------------------------------------------------------

#[query]
#[derive(Debug)]
pub struct DeleteTunerQuery {
    pub id: Uuid,
}

#[delete("/livetv/tunerhosts")]
pub async fn livetv_delete_tuner_host(
    _state: State<AppState>,
    _session: AdminSession,
    _q: Query<DeleteTunerQuery>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_IMPLEMENTED)
}

// --------------------------------------------------------------------------
// GET /livetv/tunerhosts/default
// --------------------------------------------------------------------------

#[get("/livetv/tunerhosts/default")]
pub async fn livetv_tuner_host_default(
    _state: State<AppState>,
    _session: AdminSession,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse> {
    let ty = q
        .get("type")
        .cloned()
        .unwrap_or_else(|| "m3u".to_string());
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
    let sources = db::EpgSource::get_all(
        &state
            .ctx
            .db,
    )
    .await?;
    Ok(Json(
        sources
            .iter()
            .map(epg_source_to_dto)
            .collect::<Vec<_>>(),
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
    source
        .save(
            &state
                .ctx
                .db,
        )
        .await?;
    Ok((StatusCode::OK, Json(epg_source_to_dto(&source))))
}

// --------------------------------------------------------------------------
// DELETE /remux/iptv/epgsources  (?id=...)
// --------------------------------------------------------------------------

#[query]
#[derive(Debug)]
pub struct DeleteEpgQuery {
    pub id: Uuid,
}

#[delete("/remux/iptv/epgsources")]
pub async fn remux_delete_epg_source(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<DeleteEpgQuery>,
) -> Result<impl IntoResponse> {
    db::EpgSource::delete(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// GET /remux/iptv/channels  (all channels, including disabled)
// --------------------------------------------------------------------------

#[query]
#[derive(Debug, Default)]
pub struct GetAllChannelsQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub enabled: Option<bool>,
    pub country: Option<String>,
    pub group: Option<String>,
    pub sort: Option<String>,
}

#[get("/remux/iptv/channels")]
pub async fn remux_iptv_channels(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(q): Query<GetAllChannelsQuery>,
) -> Result<impl IntoResponse> {
    let sort_by = match q
        .sort
        .as_deref()
    {
        Some("name") => vec![api::ItemSortBy::SortName],
        _ => vec![api::ItemSortBy::ChannelOrder],
    };
    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::TvChannel]),
            limit: q.limit,
            offset: q.offset,
            title_contains: q
                .search
                .filter(|s| !s.is_empty()),
            enabled: q.enabled,
            country_filter: q
                .country
                .filter(|s| !s.is_empty()),
            iptv_group_filter: q
                .group
                .filter(|s| !s.is_empty()),
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
    .fetch_all(
        &state
            .ctx
            .db,
    )
    .await?;
    Ok(Json(rows))
}

// --------------------------------------------------------------------------
// GET /remux/iptv/channels/groups  (distinct group values for TvChannels)
// --------------------------------------------------------------------------

#[get("/remux/iptv/channels/groups")]
pub async fn remux_iptv_channel_groups(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<impl IntoResponse> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT json_extract(external_ids, '$.iptv_group') AS grp FROM media \
         WHERE kind = 'tv_channel' AND grp IS NOT NULL AND grp != '' \
         ORDER BY grp",
    )
    .fetch_all(
        &state
            .ctx
            .db,
    )
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
    if let Some(search) = body
        .search
        .filter(|s| !s.is_empty())
    {
        sqlx::query(
            "UPDATE media SET enabled = $1, updated_at = datetime('now')
             WHERE kind = 'tv_channel' AND (title LIKE $2 OR custom_name LIKE $2)",
        )
        .bind(enabled_val)
        .bind(format!("%{search}%"))
        .execute(
            &state
                .ctx
                .db,
        )
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
    .execute(
        &state
            .ctx
            .db,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct EpgSourceDto {
    pub id: String,
    pub name: String,
    pub url: String,
}

fn epg_source_to_dto(source: &db::EpgSource) -> EpgSourceDto {
    EpgSourceDto {
        id: source
            .id
            .simple()
            .to_string(),
        name: source
            .name
            .clone(),
        url: source
            .url
            .clone(),
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
        id: m
            .id
            .simple()
            .to_string(),
        name: m
            .title
            .clone(),
        custom_name: m
            .custom_name
            .clone(),
        channel_number: m.channel_number,
        sort_order: m.sort_order,
        enabled: m.enabled,
        logo: m
            .images
            .get_path(db::ImageKind::Primary)
            .map(str::to_owned),
        group: m
            .external_ids
            .iptv_group
            .clone(),
        country: m
            .country
            .clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Uuid = Uuid::from_u128(7);

    /// Dispatcharr's playlist for a recording with `n` segments so far.
    fn upstream(n: usize) -> String {
        let mut p = String::from(
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-DISCONTINUITY\n",
        );
        for i in 0..n {
            p.push_str(&format!(
                "#EXTINF:4.000000,\nhttp://d:9191/api/channels/recordings/12/hls/seg_{i:05}.ts\n"
            ));
        }
        p
    }

    /// A recording that has been running far longer than what it has produced,
    /// so there is no lead to add.
    fn grow(scheduled_secs: f64) -> Option<Growth> {
        Some(Growth {
            scheduled_secs,
            elapsed_secs: 1e9,
        })
    }

    fn segment_uris(playlist: &str) -> Vec<&str> {
        playlist
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect()
    }

    fn total_secs(playlist: &str) -> f64 {
        playlist
            .lines()
            .filter_map(|l| l.strip_prefix("#EXTINF:"))
            .filter_map(|l| {
                l.trim_end_matches(',')
                    .parse::<f64>()
                    .ok()
            })
            .sum()
    }

    #[test]
    fn snapshot_is_a_finite_vod_routed_through_remux() {
        let p = recording_vod_playlist(&upstream(3), ID, None);
        assert!(p.starts_with("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n"));
        assert!(
            p.trim_end()
                .ends_with("#EXT-X-ENDLIST")
        );
        assert_eq!(
            segment_uris(&p),
            (0..3)
                .map(|i| format!("/livetv/liverecordings/{ID}/hls/seg_{i:05}.ts"))
                .collect::<Vec<_>>()
        );
        // Dispatcharr's own tags survive.
        assert!(p.contains("#EXT-X-DISCONTINUITY"));
        assert!(!p.contains("dispatcharr") && !p.contains("9191"));
    }

    #[test]
    fn never_carries_two_playlist_types_or_endlists() {
        let live = format!(
            "{}#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-ENDLIST\n",
            upstream(2)
        );
        let p = recording_vod_playlist(&live, ID, grow(60.0));
        assert_eq!(
            p.matches("#EXT-X-PLAYLIST-TYPE")
                .count(),
            1
        );
        assert_eq!(
            p.matches("#EXT-X-ENDLIST")
                .count(),
            1
        );
        assert!(p.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    }

    #[test]
    fn pads_to_the_scheduled_length_continuing_the_numbering() {
        let p = recording_vod_playlist(&upstream(3), ID, grow(30.0));
        let uris = segment_uris(&p);
        assert_eq!(
            uris.last()
                .copied(),
            Some(format!("/livetv/liverecordings/{ID}/hls/seg_00007.ts").as_str())
        );
        assert_eq!(uris.len(), 8, "3 real + ceil(18 / 4) padded");
        assert!((total_secs(&p) - 30.0).abs() < 1e-3, "{}", total_secs(&p));
        // Every entry stays within the target duration the header promises.
        assert!(
            p.lines()
                .filter_map(|l| l.strip_prefix("#EXTINF:"))
                .all(|l| l
                    .trim_end_matches(',')
                    .parse::<f64>()
                    .unwrap()
                    <= 5.0)
        );
    }

    #[test]
    fn allows_for_the_lead_dispatcharrs_recording_runs_ahead_of_the_clock() {
        // 5 segments = 20 s of content only 2 s after the scheduled start: the
        // recording is 18 s ahead, so it will end 18 s past its schedule.
        let growth = Growth {
            scheduled_secs: 30.0,
            elapsed_secs: 2.0,
        };
        let p = recording_vod_playlist(&upstream(5), ID, Some(growth));
        assert!((total_secs(&p) - 48.0).abs() < 1e-3, "{}", total_secs(&p));
    }

    #[test]
    fn a_recording_behind_the_clock_gets_no_lead() {
        let growth = Growth {
            scheduled_secs: 30.0,
            elapsed_secs: 100.0,
        };
        let p = recording_vod_playlist(&upstream(5), ID, Some(growth));
        assert!((total_secs(&p) - 30.0).abs() < 1e-3, "{}", total_secs(&p));
    }

    #[test]
    fn does_not_pad_when_already_long_enough() {
        let p = recording_vod_playlist(&upstream(3), ID, grow(12.2));
        assert_eq!(segment_uris(&p).len(), 3);
        let p = recording_vod_playlist(&upstream(3), ID, grow(5.0));
        assert_eq!(segment_uris(&p).len(), 3);
    }

    #[test]
    fn a_recording_with_no_segments_yet_starts_from_zero() {
        let p = recording_vod_playlist(&upstream(0), ID, grow(12.0));
        assert_eq!(
            segment_uris(&p),
            (0..3)
                .map(|i| format!("/livetv/liverecordings/{ID}/hls/seg_{i:05}.ts"))
                .collect::<Vec<_>>(),
            "falls back to TARGETDURATION (5 s) per segment"
        );
    }

    #[test]
    fn an_unrecognised_segment_name_is_left_as_a_snapshot() {
        let odd =
            "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:4.0,\nhttp://d/hls/chunk.ts\n";
        let p = recording_vod_playlist(odd, ID, grow(60.0));
        assert_eq!(segment_uris(&p).len(), 1);
        assert!(
            p.trim_end()
                .ends_with("#EXT-X-ENDLIST")
        );
    }

    #[test]
    fn a_bogus_schedule_cannot_build_an_unbounded_playlist() {
        let p = recording_vod_playlist(&upstream(1), ID, grow(1e12));
        assert_eq!(segment_uris(&p).len() as u64, 1 + MAX_PADDED_SEGMENTS);
    }

    // -- waiting for a segment ----------------------------------------

    use std::time::{Duration as StdDuration, Instant};

    const POLL: StdDuration = StdDuration::from_millis(20);

    #[tokio::test]
    async fn returns_at_once_when_the_segment_exists() {
        let server = httpmock::MockServer::start();
        let head = server.mock(|when, then| {
            when.method(httpmock::Method::HEAD)
                .path("/seg_00001.ts")
                .header("X-API-Key", "k");
            then.status(200);
        });
        let started = Instant::now();
        wait_for_segment(
            &server.url("/seg_00001.ts"),
            "k",
            StdDuration::from_secs(10),
            POLL,
            || async { true },
        )
        .await;
        assert!(started.elapsed() < StdDuration::from_secs(2));
        head.assert_hits(1);
    }

    #[tokio::test]
    async fn keeps_polling_until_the_segment_appears() {
        let server = httpmock::MockServer::start();
        let mut missing = server.mock(|when, then| {
            when.method(httpmock::Method::HEAD)
                .path("/seg_00009.ts");
            then.status(404);
        });
        let url = server.url("/seg_00009.ts");
        let waiter = tokio::spawn(async move {
            let started = Instant::now();
            wait_for_segment(&url, "k", StdDuration::from_secs(10), POLL, || async {
                true
            })
            .await;
            started.elapsed()
        });
        tokio::time::sleep(StdDuration::from_millis(300)).await;
        missing.delete();
        server.mock(|when, then| {
            when.method(httpmock::Method::HEAD)
                .path("/seg_00009.ts");
            then.status(200);
        });
        let waited = waiter
            .await
            .unwrap();
        assert!(waited >= StdDuration::from_millis(250), "{waited:?}");
        assert!(waited < StdDuration::from_secs(5), "{waited:?}");
    }

    #[tokio::test]
    async fn gives_up_after_the_wait_limit() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::HEAD)
                .path("/seg_00042.ts");
            then.status(404);
        });
        let started = Instant::now();
        wait_for_segment(
            &server.url("/seg_00042.ts"),
            "k",
            StdDuration::from_millis(300),
            POLL,
            || async { true },
        )
        .await;
        let waited = started.elapsed();
        assert!(waited >= StdDuration::from_millis(300), "{waited:?}");
        assert!(waited < StdDuration::from_secs(3), "{waited:?}");
    }

    #[tokio::test]
    async fn stops_waiting_once_the_recording_is_no_longer_running() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::HEAD)
                .path("/seg_00042.ts");
            then.status(404);
        });
        let started = Instant::now();
        wait_for_segment(
            &server.url("/seg_00042.ts"),
            "k",
            StdDuration::from_secs(30),
            POLL,
            || async { false },
        )
        .await;
        assert!(started.elapsed() < StdDuration::from_secs(3));
    }
}
