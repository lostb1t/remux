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

/// Resolves a cached search result from the store into a persisted `db::Media`.
///
/// - Movie/Series: resolves IMDB ID first (via TMDB), then saves.
/// - Track/Album: builds artist root from `external_ids.deezer_artist`, runs
///   `process_meta_item` which triggers `sync_tree` → full discography.
/// - Artist/Person: passed directly to `process_meta_item`.
async fn persist_from_store(
    id: Uuid,
    ctx: &AppContext,
) -> anyhow::Result<Option<db::Media>> {
    let Some(mut media) = ctx.store.get::<db::Media>(id.to_string()) else {
        return Ok(None);
    };
    ctx.store.delete(id.to_string());

    if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
        if !crate::services::imdb::resolve_media_imdb(&mut media, ctx).await {
            tracing::warn!(%id, kind = ?media.kind, "persist_from_store: IMDB resolution failed, saving without IMDB ID");
        }
    }

    let root = if matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album) {
        let Some(deezer_artist_id) = media.external_ids.deezer_artist else {
            tracing::debug!(%id, kind = ?media.kind, "persist_from_store: no deezer_artist id on music child");
            return Ok(None);
        };
        db::Media {
            id: crate::common::get_stable_uuid(format!("artist:{}", deezer_artist_id)),
            title: media.series_title.clone().unwrap_or_default(),
            kind: db::MediaKind::Artist,
            media_id: Some(deezer_artist_id.to_string()),
            ..Default::default()
        }
    } else {
        media
    };

    // Save the root stub before process_meta_item so apply_meta's MediaRelation::upsert
    // can write media_relations rows without hitting the left_media_id FK constraint.
    db::Media::upsert(&ctx.db, &[root.clone()]).await.ok();

    let processed = ctx.addons.process_meta_item(root, ctx, false).await;
    if !processed.is_empty() {
        db::Media::upsert(&ctx.db, &processed).await.ok();
    }
    Ok(db::Media::get_by_id(&ctx.db, &id).await?)
}

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
                persist_from_store(id, ctx).await.ok();
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

/// Core locking logic, injectable for testing.
///
/// `lookup` — check whether the item is already persisted.
/// `persist` — do the expensive addon persist; called at most once per ID across all concurrent
///             callers.
async fn resolve_item_core<L, P, LFut, PFut>(
    id: Uuid,
    lookup: L,
    persist: P,
) -> anyhow::Result<Option<db::Media>>
where
    L: Fn() -> LFut,
    LFut: std::future::Future<Output = anyhow::Result<Option<db::Media>>>,
    P: Fn() -> PFut,
    PFut: std::future::Future<Output = anyhow::Result<Option<db::Media>>>,
{
    if let Some(media) = lookup().await? {
        return Ok(Some(media));
    }

    let lock = persist_locks()
        .entry(id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();

    let result = {
        let _guard = lock.lock().await;
        if let Some(media) = lookup().await? {
            Ok(Some(media))
        } else {
            persist().await
        }
        // _guard dropped here — waiters unblock and hit the re-check above
    };

    persist_locks().remove(&id);
    result
}

/// Resolves a synthetic item ID to a real `db::Media` row, persisting it via addons if needed.
///
/// Uses a per-ID mutex to prevent duplicate persists from concurrent requests.
pub(crate) async fn resolve_item(
    id: Uuid,
    ctx: &AppContext,
) -> anyhow::Result<Option<db::Media>> {
    resolve_item_core(
        id,
        || async { Ok(db::Media::get_by_id(&ctx.db, &id).await?) },
        || persist_from_store(id, ctx),
    )
    .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Barrier;

    fn make_media() -> db::Media {
        db::Media {
            id: Uuid::new_v4(),
            title: "Test".into(),
            ..Default::default()
        }
    }

    // --- fast path ---

    #[tokio::test]
    async fn fast_path_returns_existing() {
        let id = Uuid::new_v4();
        let media = make_media();
        let m = media.clone();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let pc = persist_calls.clone();

        let result = resolve_item_core(
            id,
            move || {
                let m = m.clone();
                async move { Ok(Some(m)) }
            },
            move || {
                pc.fetch_add(1, Ordering::SeqCst);
                async { Ok(None) }
            },
        )
        .await
        .unwrap();

        assert_eq!(result.unwrap().id, media.id);
        assert_eq!(
            persist_calls.load(Ordering::SeqCst),
            0,
            "persist must not be called"
        );
    }

    // --- slow path: item not in DB, persist saves it ---

    #[tokio::test]
    async fn slow_path_calls_persist_once() {
        let id = Uuid::new_v4();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let pc = persist_calls.clone();
        let media = make_media();
        let m = media.clone();

        let result = resolve_item_core(
            id,
            || async { Ok(None) },
            move || {
                pc.fetch_add(1, Ordering::SeqCst);
                let m = m.clone();
                async move { Ok(Some(m)) }
            },
        )
        .await
        .unwrap();

        assert_eq!(result.unwrap().id, media.id);
        assert_eq!(persist_calls.load(Ordering::SeqCst), 1);
    }

    // --- slow path: persist returns None ---

    #[tokio::test]
    async fn slow_path_propagates_none() {
        let id = Uuid::new_v4();
        let result =
            resolve_item_core(id, || async { Ok(None) }, || async { Ok(None) })
                .await
                .unwrap();
        assert!(result.is_none());
    }

    // --- error path: lock is always cleaned up ---

    #[tokio::test]
    async fn error_path_cleans_up_lock() {
        let id = Uuid::new_v4();
        let _ = resolve_item_core(
            id,
            || async { Ok(None) },
            || async { Err(anyhow::anyhow!("persist failed")) },
        )
        .await;

        assert!(
            !persist_locks().contains_key(&id),
            "lock must be removed even after error"
        );
    }

    // --- concurrent: second waiter must not call persist ---

    #[tokio::test]
    async fn concurrent_second_waiter_skips_persist() {
        let id = Uuid::new_v4();
        let persist_calls = Arc::new(AtomicUsize::new(0));
        let saved = Arc::new(tokio::sync::RwLock::new(false));

        // Barrier lets both tasks start at the same time.
        let barrier = Arc::new(Barrier::new(2));

        let (pc1, saved1, b1) = (persist_calls.clone(), saved.clone(), barrier.clone());
        let t1 = tokio::spawn(async move {
            b1.wait().await;
            resolve_item_core(
                id,
                {
                    let saved = saved1.clone();
                    move || {
                        let s = saved.clone();
                        async move {
                            Ok(if *s.read().await {
                                Some(make_media())
                            } else {
                                None
                            })
                        }
                    }
                },
                move || {
                    pc1.fetch_add(1, Ordering::SeqCst);
                    let saved = saved1.clone();
                    async move {
                        // Simulate slow persist.
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        *saved.write().await = true;
                        Ok(Some(make_media()))
                    }
                },
            )
            .await
        });

        let (pc2, saved2, b2) = (persist_calls.clone(), saved.clone(), barrier.clone());
        let t2 = tokio::spawn(async move {
            b2.wait().await;
            resolve_item_core(
                id,
                {
                    let saved = saved2.clone();
                    move || {
                        let s = saved.clone();
                        async move {
                            Ok(if *s.read().await {
                                Some(make_media())
                            } else {
                                None
                            })
                        }
                    }
                },
                move || {
                    pc2.fetch_add(1, Ordering::SeqCst);
                    let saved = saved2.clone();
                    async move {
                        *saved.write().await = true;
                        Ok(Some(make_media()))
                    }
                },
            )
            .await
        });

        let (r1, r2) = tokio::join!(t1, t2);
        assert!(r1.unwrap().unwrap().is_some(), "t1 must resolve");
        assert!(r2.unwrap().unwrap().is_some(), "t2 must resolve");
        assert_eq!(
            persist_calls.load(Ordering::SeqCst),
            1,
            "persist must be called exactly once across both concurrent requests"
        );
    }
}
