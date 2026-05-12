use anyhow::Result as AnyResult;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use remux_macros::{delete, get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::db::{self, auth};
use crate::{AppState, sdks};
use axum_anyhow::ApiResult as Result;
use uuid::Uuid;

const CACHE_KEY_PREFIX: &str = "remux:cache:";

#[derive(Debug, Deserialize, Default)]
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
    id.strip_prefix("request-").unwrap_or(id).to_string()
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
    .fetch_all(&state.ctx.db)
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
        let expired = parsed.as_ref().map(is_expired).unwrap_or(false);

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
            .execute(&state.ctx.db)
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

    let Some(record) = load_cache_record(&state.ctx.db, &ns, &key).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

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
        &state.ctx.db,
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
    delete_cache_record(&state.ctx.db, &query.ns, &key).await?;
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
        if let Some(record) = load_cache_record(&state.ctx.db, &query.ns, &key).await? {
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

/// Source-level metadata (binge group, name, description) for an item.
/// Returns stream metadata from `/gelato/streams/{id}` so the
/// client can match versions across episodes without re-issuing playback info.
async fn streams_metadata(state: &AppState, id: Uuid) -> AnyResult<StreamsResponse> {
    let Some(media) = db::Media::get_by_id(&state.ctx.db, &id).await? else {
        return Ok(StreamsResponse { streams: vec![] });
    };

    // Accept both the parent (Movie/Episode) and the Source itself.
    let mut parent = if media.kind == db::MediaKind::Stream {
        media.parent(&state.ctx.db).await?.unwrap_or(media)
    } else {
        media
    };

    if !matches!(
        parent.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        return Ok(StreamsResponse { streams: vec![] });
    }

    let mut sources = parent.streams(&state.ctx.db).await.unwrap_or_default();
    sources.sort_by_key(|s| s.idx.unwrap_or(0));

    let streams = sources
        .into_iter()
        .map(|s| {
            // Title in the DB carries the merged "name\ndescription" string —
            // split it back so legacy clients that only render `Name` still
            // see something useful and ones that consume both get the pair.
            let (name, description) = match s.title.split_once('\n') {
                Some((n, d)) => {
                    (Some(n.trim().to_string()), Some(d.trim().to_string()))
                }
                None if !s.title.is_empty() => (Some(s.title.clone()), None),
                _ => (None, None),
            };
            StreamMetadataDto {
                id: s.id,
                name,
                description,
                index: s.idx.unwrap_or(0),
                size: s.probe_data.as_ref().and_then(|p| p.size),
            }
        })
        .collect();

    Ok(StreamsResponse { streams })
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

    let manifest_url = match crate::addons::Addon::list(&state.ctx.db).await {
        Ok(addons) => addons
            .into_iter()
            .filter(|a| a.enabled && a.preset.kind == "stremio")
            .find_map(|a| {
                a.preset
                    .config
                    .get("manifest_url")
                    .and_then(|v| v.as_str().map(str::to_string))
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

    match svc.get_meta(media_type, id).await {
        Ok(meta) => Ok(Json::<remux_sdks::stremio::Meta>(meta).into_response()),
        Err(_) => {
            Ok((StatusCode::NOT_FOUND, Json(serde_json::Value::Null)).into_response())
        }
    }
}
