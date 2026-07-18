use anyhow::Result as AnyResult;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use remux_macros::{delete, get, post, query};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::{
    AppState, OptionExt,
    db::{self, auth},
    sdks,
};
use axum_anyhow::ApiResult as Result;
use uuid::Uuid;

const CACHE_KEY_PREFIX: &str = "remux:cache:";

#[query]
#[derive(Debug, Default)]
pub struct NamespaceQuery {
    #[serde(default)]
    pub ns: String,
}

#[derive(Debug, Deserialize)]
pub struct CacheSetRequest {
    #[serde(rename = "Value", alias = "value")]
    pub value: String,
    #[serde(rename = "TtlSeconds", alias = "ttlSeconds", default)]
    pub ttl_seconds: i64,
}

#[derive(Debug, Deserialize, Default)]
pub struct CacheBulkRequest {
    #[serde(default, alias = "Keys")]
    pub keys: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CacheGetResponse {
    pub value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CacheStatsResponse {
    pub total_keys: usize,
    pub active_keys: usize,
    pub expired_keys: usize,
    pub namespaces: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheRecord {
    value: String,
    expires_at: Option<DateTime<Utc>>,
}

fn encode_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn decode_component(value: &str) -> String {
    let wrapped = format!("v={value}");
    url::form_urlencoded::parse(wrapped.as_bytes())
        .find_map(|(k, v)| if k == "v" { Some(v.into_owned()) } else { None })
        .unwrap_or_else(|| value.to_string())
}

fn cache_storage_key(ns: &str, key: &str) -> String {
    format!(
        "{CACHE_KEY_PREFIX}{}:{}",
        encode_component(ns),
        encode_component(key)
    )
}

fn parse_cache_storage_key(storage_key: &str) -> Option<(String, String)> {
    if !storage_key.starts_with(CACHE_KEY_PREFIX) {
        return None;
    }

    let rest = &storage_key[CACHE_KEY_PREFIX.len()..];
    let (ns_enc, key_enc) = rest.split_once(':')?;
    Some((decode_component(ns_enc), decode_component(key_enc)))
}

fn normalize_registration_id(id: &str) -> String {
    id.strip_prefix("request-")
        .unwrap_or(id)
        .to_string()
}

fn compute_expiration(ttl_seconds: i64) -> Option<DateTime<Utc>> {
    if ttl_seconds <= 0 {
        return None;
    }

    // Clamp to avoid pathological values.
    let ttl = ttl_seconds.clamp(1, 60 * 60 * 24 * 365 * 10);
    Utc::now().checked_add_signed(Duration::seconds(ttl))
}

fn is_expired(record: &CacheRecord) -> bool {
    match record.expires_at {
        Some(ts) => ts <= Utc::now(),
        None => false,
    }
}

async fn delete_cache_record(
    db_pool: &sqlx::SqlitePool,
    ns: &str,
    key: &str,
) -> AnyResult<()> {
    let storage_key = cache_storage_key(ns, key);
    sqlx::query("DELETE FROM settings WHERE key = ?1")
        .bind(storage_key)
        .execute(db_pool)
        .await?;
    Ok(())
}

async fn load_cache_record(
    db_pool: &sqlx::SqlitePool,
    ns: &str,
    key: &str,
) -> AnyResult<Option<CacheRecord>> {
    let storage_key = cache_storage_key(ns, key);
    let Some(raw) = db::Settings::get(db_pool, &storage_key).await? else {
        return Ok(None);
    };

    let record = match serde_json::from_str::<CacheRecord>(&raw) {
        Ok(parsed) => parsed,
        Err(_) => CacheRecord {
            value: raw,
            expires_at: None,
        },
    };

    if is_expired(&record) {
        delete_cache_record(db_pool, ns, key).await?;
        return Ok(None);
    }

    Ok(Some(record))
}

async fn save_cache_record(
    db_pool: &sqlx::SqlitePool,
    ns: &str,
    key: &str,
    value: &str,
    ttl_seconds: i64,
) -> AnyResult<()> {
    let record = CacheRecord {
        value: value.to_string(),
        expires_at: compute_expiration(ttl_seconds),
    };

    let storage_key = cache_storage_key(ns, key);
    let raw = serde_json::to_string(&record)?;
    db::Settings::set(db_pool, &storage_key, &raw).await
}

#[get("/remux/cache/stats")]
pub async fn remux_cache_stats(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<Response> {
    let pattern = format!("{CACHE_KEY_PREFIX}%");
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM settings WHERE key LIKE ?1",
    )
    .bind(pattern)
    .fetch_all(
        &state
            .ctx
            .db,
    )
    .await?;

    let mut namespaces = HashSet::new();
    let mut active_keys = 0usize;
    let mut expired_keys = 0usize;
    let mut expired_storage_keys = Vec::new();

    for (storage_key, raw_value) in &rows {
        if let Some((ns, _)) = parse_cache_storage_key(storage_key) {
            namespaces.insert(ns);
        }

        let parsed = serde_json::from_str::<CacheRecord>(raw_value).ok();
        let expired = parsed
            .as_ref()
            .map(is_expired)
            .unwrap_or(false);

        if expired {
            expired_keys += 1;
            expired_storage_keys.push(storage_key.clone());
        } else {
            active_keys += 1;
        }
    }

    for storage_key in expired_storage_keys {
        sqlx::query("DELETE FROM settings WHERE key = ?1")
            .bind(storage_key)
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await?;
    }

    let payload = CacheStatsResponse {
        total_keys: rows.len(),
        active_keys,
        expired_keys,
        namespaces: namespaces.len(),
    };

    Ok((StatusCode::OK, Json(payload)).into_response())
}

#[get("/remux/cache/{key}")]
pub async fn remux_cache_get(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(key): Path<String>,
    Query(query): Query<NamespaceQuery>,
) -> Result<Response> {
    let ns = query.ns;

    let record = load_cache_record(
        &state
            .ctx
            .db,
        &ns,
        &key,
    )
    .await?
    .context_not_found("not found")?;

    Ok((
        StatusCode::OK,
        Json(CacheGetResponse {
            value: record.value,
        }),
    )
        .into_response())
}

#[post("/remux/cache/{key}")]
pub async fn remux_cache_set(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(key): Path<String>,
    Query(query): Query<NamespaceQuery>,
    Json(payload): Json<CacheSetRequest>,
) -> Result<Response> {
    save_cache_record(
        &state
            .ctx
            .db,
        &query.ns,
        &key,
        &payload.value,
        payload.ttl_seconds,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[delete("/remux/cache/{key}")]
pub async fn remux_cache_delete(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(key): Path<String>,
    Query(query): Query<NamespaceQuery>,
) -> Result<Response> {
    delete_cache_record(
        &state
            .ctx
            .db,
        &query.ns,
        &key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/remux/cache/bulk")]
pub async fn remux_cache_bulk(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(query): Query<NamespaceQuery>,
    Json(payload): Json<CacheBulkRequest>,
) -> Result<Response> {
    let mut out = HashMap::<String, String>::new();

    for key in payload.keys {
        if let Some(record) = load_cache_record(
            &state
                .ctx
                .db,
            &query.ns,
            &key,
        )
        .await?
        {
            out.insert(key, record.value);
        }
    }

    Ok((StatusCode::OK, Json(out)).into_response())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamMetadataDto {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub index: i64,
    pub size: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamsResponse {
    pub streams: Vec<StreamMetadataDto>,
}

fn source_to_dto(s: &db::Media) -> StreamMetadataDto {
    // Title in the DB carries the merged "name\ndescription" string —
    // split it back so legacy clients that only render `Name` still
    // see something useful and ones that consume both get the pair.
    let (name, description) = match s
        .title
        .split_once('\n')
    {
        Some((n, d)) => (
            Some(
                n.trim()
                    .to_string(),
            ),
            Some(
                d.trim()
                    .to_string(),
            ),
        ),
        None if !s
            .title
            .is_empty() =>
        {
            (
                Some(
                    s.title
                        .clone(),
                ),
                None,
            )
        }
        _ => (None, None),
    };
    StreamMetadataDto {
        id: s.id,
        name,
        description,
        index: s
            .idx
            .unwrap_or(0),
        size: s
            .probe_data
            .as_ref()
            .and_then(|p| p.size),
    }
}

/// Source-level metadata (binge group, name, description) for an item.
/// Returns stream metadata from `/gelato/streams/{id}` so the
/// client can match versions across episodes without re-issuing playback info.
async fn streams_metadata(state: &AppState, id: Uuid) -> AnyResult<StreamsResponse> {
    let Some(media) = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    else {
        return Ok(StreamsResponse { streams: vec![] });
    };

    // Accept both the parent (Movie/Episode) and the Source itself.
    let mut parent = if media.kind == db::MediaKind::Stream {
        media
            .parent(
                &state
                    .ctx
                    .db,
            )
            .await?
            .unwrap_or(media)
    } else {
        media
    };

    if !matches!(
        parent.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        return Ok(StreamsResponse { streams: vec![] });
    }

    let mut sources = parent
        .streams(
            &state
                .ctx
                .db,
        )
        .await
        .unwrap_or_default();
    sources.sort_by_key(|s| {
        s.idx
            .unwrap_or(0)
    });

    let groups = db::StreamGroup::list(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    let enabled_groups: Vec<&db::StreamGroup> = groups
        .iter()
        .filter(|g| g.enabled)
        .collect();

    if enabled_groups.is_empty() {
        let streams = sources
            .iter()
            .map(source_to_dto)
            .collect();
        return Ok(StreamsResponse { streams });
    }

    let config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = config
        .stream_groups_show_ungrouped
        .unwrap_or(true);

    let mut result: Vec<StreamMetadataDto> = vec![];
    let mut matched_ids: HashSet<Uuid> = HashSet::new();

    for group in &enabled_groups {
        let matching: Vec<&db::Media> = sources
            .iter()
            .filter(|s| {
                s.stream_info
                    .as_ref()
                    .map_or(false, |info| group.matches(info))
            })
            .collect();

        if matching.is_empty() {
            continue;
        }

        for s in &matching {
            matched_ids.insert(s.id);
        }

        let best = matching[0];
        let description = {
            use remux_sdks::remux::StreamRule;
            let parts: Vec<String> = group
                .filter
                .rules
                .iter()
                .map(|r| match r {
                    StreamRule::Resolution { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Quality { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                    StreamRule::Codec { values, .. } => values
                        .iter()
                        .map(|v| v.label())
                        .collect::<Vec<_>>()
                        .join("/"),
                })
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" · "))
            }
        };
        result.push(StreamMetadataDto {
            id: best.id,
            name: Some(group.display_name()),
            description,
            index: result.len() as i64,
            size: best
                .probe_data
                .as_ref()
                .and_then(|p| p.size),
        });
    }

    if show_ungrouped {
        for s in &sources {
            if !matched_ids.contains(&s.id) {
                result.push(source_to_dto(s));
            }
        }
    }

    Ok(StreamsResponse { streams: result })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetricsStatusResponse {
    pub daily_days: i64,
    pub daily_window: i64,
    pub last_updated_days_ago: Option<i64>,
    pub item_count: i64,
}

#[get("/remux/metrics/status")]
pub async fn remux_metrics_status(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let row = sqlx::query_as::<_, (i64, Option<i64>, i64)>(
        "SELECT COUNT(DISTINCT period_key), \
                CAST(julianday('now') - julianday(MAX(period_key)) AS INTEGER), \
                COUNT(DISTINCT media_id) \
         FROM popularity_agg \
         WHERE period = 'daily' AND period_key >= date('now', '-14 days')",
    )
    .fetch_one(
        &state
            .ctx
            .db,
    )
    .await?;

    Ok(Json(MetricsStatusResponse {
        daily_days: row.0,
        daily_window: 14,
        last_updated_days_ago: row.1,
        item_count: row.2,
    }))
}

/// Per-endpoint latency snapshot for every route seen since startup, sorted by
/// total time spent. Admin-gated and only available when `metrics_enabled` is
/// set in config; returns 404 otherwise so it stays invisible in production.
#[get("/remux/metrics")]
pub async fn remux_metrics(
    State(state): State<AppState>,
    _admin: auth::AdminSession,
) -> Result<Response> {
    if !state
        .ctx
        .config
        .metrics_enabled
    {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let routes = state
        .ctx
        .metrics
        .snapshot();
    Ok(Json(json!({ "routes": routes })).into_response())
}

/// Jellyflix playback milestones. Authentication supplies the authoritative
/// user/device identity; clients only provide playback context and timings.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryPlaybackEventRequest {
    pub playback_key: String,
    pub event: String,
    pub elapsed_ms: Option<f64>,
    pub item_id: Option<String>,
    pub item_name: Option<String>,
    pub series_name: Option<String>,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub delivery_class: Option<String>,
    pub error_category: Option<String>,
    pub details: Option<serde_json::Value>,
}

#[post("/remux/telemetry/playback")]
pub async fn telemetry_playback_event(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(event): Json<TelemetryPlaybackEventRequest>,
) -> Result<impl IntoResponse> {
    if !state
        .ctx
        .config
        .telemetry_enabled
    {
        return Ok(StatusCode::NO_CONTENT);
    }
    let playback_key = event
        .playback_key
        .trim();
    let event_name = event
        .event
        .trim();
    if playback_key.is_empty()
        || event_name.is_empty()
        || playback_key.len() > 160
        || event_name.len() > 120
    {
        return Ok(StatusCode::BAD_REQUEST);
    }
    let details = event
        .details
        .map(|value| {
            // Bound the diagnostic payload and never preserve arbitrary large data.
            let mut text = value.to_string();
            text.truncate(2_000);
            text
        });
    sqlx::query(
        "INSERT INTO telemetry_playback_events \
         (playback_key, event, elapsed_ms, item_id, item_name, series_name, source_id, source_name, delivery_class, error_category, user_id, device_id, device_name, client_name, client_version, details_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(playback_key)
    .bind(event_name)
    .bind(event.elapsed_ms.filter(|value| value.is_finite() && *value >= 0.0))
    .bind(event.item_id.map(|value| value.chars().take(120).collect::<String>()))
    .bind(event.item_name.map(|value| value.chars().take(500).collect::<String>()))
    .bind(event.series_name.map(|value| value.chars().take(500).collect::<String>()))
    .bind(event.source_id.map(|value| value.chars().take(160).collect::<String>()))
    .bind(event.source_name.map(|value| value.chars().take(500).collect::<String>()))
    .bind(event.delivery_class.map(|value| value.chars().take(80).collect::<String>()))
    .bind(event.error_category.map(|value| value.chars().take(120).collect::<String>()))
    .bind(session.user.id.to_string())
    .bind(&session.device.id)
    .bind(&session.device.name)
    .bind(&session.device.app_name)
    .bind(&session.device.app_version)
    .bind(details)
    .execute(&state.ctx.db)
    .await?;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryOverviewResponse {
    pub since: String,
    pub request_count: i64,
    pub error_count: i64,
    pub mean_latency_ms: f64,
    pub max_latency_ms: f64,
    pub playback_events: i64,
    pub startup_failures: i64,
}

/// Compact default overview; detailed graph/filter endpoints can build on the
/// raw tables without exposing them to non-admin users.
#[get("/remux/telemetry/overview")]
pub async fn telemetry_overview(
    State(state): State<AppState>,
    _admin: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let row = sqlx::query_as::<_, (i64, i64, f64, f64)>(
        "SELECT COUNT(*), SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END), COALESCE(AVG(latency_ms), 0), COALESCE(MAX(latency_ms), 0) \
         FROM telemetry_request_events WHERE created_at >= datetime('now', '-24 hours')"
    ).fetch_one(&state.ctx.db).await?;
    let playback = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), SUM(CASE WHEN event IN ('startup-timeout', 'error') THEN 1 ELSE 0 END) \
         FROM telemetry_playback_events WHERE created_at >= datetime('now', '-24 hours')"
    ).fetch_one(&state.ctx.db).await?;
    Ok(Json(TelemetryOverviewResponse {
        since: "24h".to_string(),
        request_count: row.0,
        error_count: row.1,
        mean_latency_ms: row.2,
        max_latency_ms: row.3,
        playback_events: playback.0,
        startup_failures: playback.1,
    }))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryRankingQuery {
    pub dimension: Option<String>,
    pub hours: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryRankingRow {
    pub label: String,
    pub count: i64,
    pub error_count: i64,
    pub mean_latency_ms: f64,
    pub max_latency_ms: f64,
}

/// Admin drill-down ranking for the dimensions operators use to identify slow
/// endpoints, devices, clients, users, and content. The column is whitelisted.
#[get("/remux/telemetry/rankings")]
pub async fn telemetry_rankings(
    State(state): State<AppState>,
    _admin: auth::AdminSession,
    Query(query): Query<TelemetryRankingQuery>,
) -> Result<impl IntoResponse> {
    let column = match query
        .dimension
        .as_deref()
    {
        Some("device") => "device_name",
        Some("client") => "client_name",
        Some("user") => "user_id",
        Some("content") => "item_id",
        _ => "route_template",
    };
    let hours = query
        .hours
        .unwrap_or(24)
        .clamp(1, 24 * 30);
    let limit = query
        .limit
        .unwrap_or(25)
        .clamp(1, 200);
    let sql = format!(
        "SELECT COALESCE({column}, 'Unknown') AS label, COUNT(*) AS count, \
         SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END) AS error_count, \
         COALESCE(AVG(latency_ms), 0) AS mean_latency_ms, COALESCE(MAX(latency_ms), 0) AS max_latency_ms \
         FROM telemetry_request_events WHERE created_at >= datetime('now', ?1) \
         GROUP BY {column} ORDER BY mean_latency_ms DESC, count DESC LIMIT ?2"
    );
    let window = format!("-{} hours", hours);
    let rows = sqlx::query_as::<_, TelemetryRankingRow>(&sql)
        .bind(window)
        .bind(limit)
        .fetch_all(
            &state
                .ctx
                .db,
        )
        .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTelemetryViewRequest {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySavedView {
    pub id: String,
    pub name: String,
    pub config_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[get("/remux/telemetry/views")]
pub async fn list_telemetry_views(
    State(state): State<AppState>,
    _admin: auth::AdminSession,
) -> Result<impl IntoResponse> {
    Ok(Json(sqlx::query_as::<_, TelemetrySavedView>(
        "SELECT id, name, config_json, created_at, updated_at FROM telemetry_saved_views ORDER BY updated_at DESC"
    ).fetch_all(&state.ctx.db).await?))
}

#[post("/remux/telemetry/views")]
pub async fn save_telemetry_view(
    State(state): State<AppState>,
    _admin: auth::AdminSession,
    Json(view): Json<SaveTelemetryViewRequest>,
) -> Result<impl IntoResponse> {
    let name = view
        .name
        .trim();
    if name.is_empty() || name.len() > 120 {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let mut config_json = view
        .config
        .to_string();
    config_json.truncate(8_000);
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO telemetry_saved_views (id, name, config_json) VALUES (?, ?, ?) \
         ON CONFLICT(name) DO UPDATE SET config_json=excluded.config_json, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')"
    ).bind(&id).bind(name).bind(config_json).execute(&state.ctx.db).await?;
    Ok(Json(serde_json::json!({"id": id})).into_response())
}

#[get("/remux/streams/{id}")]
pub async fn remux_streams(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let payload = streams_metadata(&state, id)
        .await
        .unwrap_or(StreamsResponse { streams: vec![] });
    Ok(Json(payload))
}

#[get("/remux/meta/{kind}/{id}")]
pub async fn remux_meta(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((kind, id)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    let media_type = match kind.as_str() {
        "series" => remux_sdks::stremio::MediaType::Series,
        "movie" => remux_sdks::stremio::MediaType::Movie,
        _ => remux_sdks::stremio::MediaType::Movie,
    };

    let manifest_url = match crate::addons::Addon::list(
        &state
            .ctx
            .db,
    )
    .await
    {
        Ok(addons) => addons
            .into_iter()
            .filter(|a| {
                a.enabled
                    && a.preset
                        .kind
                        == "stremio"
            })
            .find_map(|a| {
                a.preset
                    .config
                    .get("manifest_url")
                    .and_then(|v| {
                        v.as_str()
                            .map(str::to_string)
                    })
            }),
        Err(_) => None,
    };

    let svc = match manifest_url
        .as_deref()
        .and_then(|u| crate::services::stremio::StremioService::from_url(u).ok())
    {
        Some(a) => a,
        None => {
            return Ok(
                (StatusCode::NOT_FOUND, Json(serde_json::Value::Null)).into_response()
            );
        }
    };

    match svc
        .get_meta(media_type, id)
        .await
    {
        Ok(meta) => Ok(Json::<remux_sdks::stremio::Meta>(meta).into_response()),
        Err(_) => {
            Ok((StatusCode::NOT_FOUND, Json(serde_json::Value::Null)).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use crate::integration_test::{
        AUTH_HEADER, TestGuard, auth_header_with_token, new_test_server_with_config,
    };
    use http::header::HeaderValue;

    async fn boot(
        enabled: bool,
    ) -> (axum_test::TestServer, TestGuard, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir
            .path()
            .join("remux-metrics-test.sqlite");
        let (server, guard) = new_test_server_with_config(Config {
            database_url: Some(format!("sqlite://{}?mode=rwc", db_path.display())),
            torrent_http_port: None,
            disable_dht: true,
            torrent_peer_port: None,
            metrics_enabled: enabled,
            ..Default::default()
        })
        .await
        .unwrap();
        (server, guard, temp_dir)
    }

    async fn admin_token(server: &axum_test::TestServer) -> String {
        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;
        resp.json::<serde_json::Value>()["AccessToken"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The middleware must record the matched route *template* (not the concrete
    /// URI) and the snapshot endpoint must surface it — proving `MatchedPath` is
    /// populated at the layer's position in the stack.
    #[tokio::test]
    async fn metrics_records_matched_route_template() {
        let (server, _guard, _tmp) = boot(true).await;

        server
            .get("/system/info/public")
            .await;
        server
            .get("/system/info/public")
            .await;

        let token = admin_token(&server).await;
        let resp = server
            .get("/remux/metrics")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        resp.assert_status_ok();

        let body: serde_json::Value = resp.json();
        let routes = body["routes"]
            .as_array()
            .unwrap();
        let hit = routes
            .iter()
            .find(|r| r["template"] == "/system/info/public")
            .expect("matched route template should be recorded");
        assert_eq!(hit["method"], "GET");
        assert_eq!(hit["mutation"], false);
        assert!(
            hit["count"]
                .as_u64()
                .unwrap()
                >= 2
        );
    }

    /// When `metrics_enabled` is false the endpoint is invisible (404), so it
    /// stays dark in production even though the route is always registered.
    #[tokio::test]
    async fn metrics_endpoint_gated_off_by_config() {
        let (server, _guard, _tmp) = boot(false).await;
        let token = admin_token(&server).await;
        let resp = server
            .get("/remux/metrics")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .expect_failure()
            .await;
        assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    }
}
