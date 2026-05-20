use std::sync::{Arc, OnceLock};

use axum::extract::FromRequestParts;
use axum::extract::Path;
use axum::http::request::Parts;
use axum_anyhow::{ApiError, ApiResult as Result};
use dashmap::DashMap;
use http::StatusCode;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppContext;
use crate::AppState;
use crate::db;

static PERSIST_LOCKS: OnceLock<DashMap<Uuid, Arc<Mutex<()>>>> = OnceLock::new();

fn persist_locks() -> &'static DashMap<Uuid, Arc<Mutex<()>>> {
    PERSIST_LOCKS.get_or_init(DashMap::new)
}

/// For each candidate ID: if not in DB, acquire its persist lock and persist if still missing;
/// if in DB but lock is held, wait for it. Returns true if a query retry is warranted.
pub(crate) async fn wait_for_persist(
    ids: &[Uuid],
    ctx: &AppContext,
) -> anyhow::Result<bool> {
    for &id in ids {
        let in_db = db::Media::get_by_id(&ctx.db, &id).await?.is_some();
        if !in_db {
            let lock = persist_locks()
                .entry(id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            let _guard = lock.lock().await;
            if db::Media::get_by_id(&ctx.db, &id).await?.is_none() {
                ctx.addons.persist_search_result(id, ctx).await.ok();
            }
            persist_locks().remove(&id);
            return Ok(true);
        } else if let Some(lock) = persist_locks().get(&id).map(|e| Arc::clone(&e)) {
            let _guard = lock.lock().await;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolves a synthetic item ID to a real `db::Media` row, persisting it via addons if needed.
///
/// Uses a per-ID mutex to prevent duplicate persists from concurrent requests.
pub(crate) async fn resolve_item(
    id: Uuid,
    ctx: &AppContext,
) -> anyhow::Result<Option<db::Media>> {
    if let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? {
        // Fast path: already in DB. If a persist is in flight, wait for it so the caller
        // gets a consistent view, then return the existing row.
        if let Some(lock) = persist_locks().get(&id).map(|e| Arc::clone(&e)) {
            let _guard = lock.lock().await;
        }
        return Ok(Some(media));
    }

    // Slow path: acquire lock so only one request triggers the expensive persist.
    let lock = persist_locks()
        .entry(id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Re-check under lock — a prior waiter may have just saved it.
    if let Some(media) = db::Media::get_by_id(&ctx.db, &id).await? {
        persist_locks().remove(&id);
        return Ok(Some(media));
    }

    let result = ctx.addons.persist_search_result(id, ctx).await?;
    persist_locks().remove(&id);
    Ok(result)
}

/// Axum extractor that resolves the `{id}` path parameter to a persisted `db::Media` row.
///
/// Returns 404 if the ID cannot be resolved even after attempting addon persistence.
pub(crate) struct ResolvedItem(pub db::Media);

impl FromRequestParts<AppState> for ResolvedItem {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Path(id) = Path::<Uuid>::from_request_parts(parts, state)
            .await
            .map_err(|_| {
                ApiError::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .title("Bad Request")
                    .detail("invalid item id")
                    .build()
            })?;

        let media = resolve_item(id, &state.ctx)
            .await
            .map_err(|e| {
                ApiError::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .title("Internal Error")
                    .detail("failed to resolve item")
                    .error(e)
                    .build()
            })?
            .ok_or_else(|| {
                ApiError::builder()
                    .status(StatusCode::NOT_FOUND)
                    .title("Not Found")
                    .detail("item not found")
                    .build()
            })?;

        Ok(ResolvedItem(media))
    }
}
