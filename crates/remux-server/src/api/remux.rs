use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use anyhow::Result as AnyResult;
use chrono::{DateTime, Duration, Utc};
use remux_macros::{delete, get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::AppState;
use crate::db::{self, auth};
use axum_anyhow::ApiResult as Result;

const CACHE_KEY_PREFIX: &str = "remux:cache:";
const REGISTRATION_NS: &str = "anfiteatro-registration";
const REGISTRATION_ENABLED_KEY: &str = "registration-enabled";
const REGISTRATION_INDEX_KEY: &str = "requests-index";

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

#[derive(Debug, Serialize)]
pub struct RegistrationEnabledResponse {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct RegistrationRequest {
    #[serde(rename = "Id", alias = "id")]
    pub id: String,
    #[serde(rename = "Data", alias = "data")]
    pub data: String,
    #[serde(rename = "TtlSeconds", alias = "ttlSeconds", default)]
    pub ttl_seconds: i64,
}

#[derive(Debug, Serialize)]
pub struct RegistrationRequestResponse {
    pub success: bool,
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

async fn is_registration_enabled(db_pool: &sqlx::SqlitePool) -> AnyResult<bool> {
    let Some(record) =
        load_cache_record(db_pool, REGISTRATION_NS, REGISTRATION_ENABLED_KEY).await?
    else {
        return Ok(true);
    };

    Ok(serde_json::from_str::<bool>(&record.value).unwrap_or(true))
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

#[get("/remux/registration/enabled")]
pub async fn remux_registration_enabled(
    State(state): State<AppState>,
) -> Result<Response> {
    let enabled = is_registration_enabled(&state.ctx.db).await?;
    Ok((StatusCode::OK, Json(RegistrationEnabledResponse { enabled })).into_response())
}

#[post("/remux/registration/request")]
pub async fn remux_registration_request(
    State(state): State<AppState>,
    Json(payload): Json<RegistrationRequest>,
) -> Result<Response> {
    if !is_registration_enabled(&state.ctx.db).await? {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({
                "success": false,
                "error": "Registration is disabled"
            })),
        )
            .into_response());
    }

    if payload.id.trim().is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Id is required"
            })),
        )
            .into_response());
    }

    save_cache_record(
        &state.ctx.db,
        REGISTRATION_NS,
        &payload.id,
        &payload.data,
        payload.ttl_seconds,
    )
    .await?;

    let mut request_index = if let Some(index_record) =
        load_cache_record(&state.ctx.db, REGISTRATION_NS, REGISTRATION_INDEX_KEY).await?
    {
        serde_json::from_str::<Vec<String>>(&index_record.value).unwrap_or_default()
    } else {
        Vec::new()
    };

    let normalized_id = normalize_registration_id(&payload.id);
    if !request_index.iter().any(|id| id == &normalized_id) {
        request_index.push(normalized_id);
    }

    let index_json = serde_json::to_string(&request_index)?;
    save_cache_record(
        &state.ctx.db,
        REGISTRATION_NS,
        REGISTRATION_INDEX_KEY,
        &index_json,
        payload.ttl_seconds,
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(RegistrationRequestResponse { success: true }),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct EmailSendRequest {
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct EmailSendResponse {
    pub success: bool,
    pub error: Option<String>,
}

#[post("/remux/email/send")]
pub async fn remux_email_send(
    _session: auth::AdminSession,
    Json(_payload): Json<EmailSendRequest>,
) -> Result<Response> {
    // SMTP relay is optional in this compatibility shim.
    Ok((
        StatusCode::OK,
        Json(EmailSendResponse {
            success: false,
            error: Some("SMTP relay is not configured in remux-server yet".to_string()),
        }),
    )
        .into_response())
}
