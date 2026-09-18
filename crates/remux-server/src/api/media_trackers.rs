use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use chrono::{Duration, NaiveDateTime, Utc};
use http::StatusCode;
use remux_macros::{delete, get, post};
use remux_sdks::remux::{
    MediaTrackerAuthPollDto, MediaTrackerAuthStatus, MediaTrackerDeviceAuthDto,
    MediaTrackerImportRunDto, MediaTrackerImportStatus, MediaTrackerProviderDto,
    UserMediaTrackerDto,
};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt,
    addons::{
        Addon,
        media_tracker::{DeviceAuthPoll, MediaTrackerCtx},
    },
    db::{self, auth},
};

#[derive(Debug, FromRow)]
struct AuthAttempt {
    id: Uuid,
    user_id: Uuid,
    addon_id: Uuid,
    poll_token: String,
    poll_interval_seconds: i64,
    next_poll_at: NaiveDateTime,
    expires_at: NaiveDateTime,
}

#[derive(Debug, FromRow)]
struct ImportRun {
    id: Uuid,
    status: String,
    fetched_count: i64,
    matched_count: i64,
    updated_count: i64,
    deferred_count: i64,
    skipped_count: i64,
    error: Option<String>,
    created_at: NaiveDateTime,
    started_at: Option<NaiveDateTime>,
    completed_at: Option<NaiveDateTime>,
}

impl ImportRun {
    fn dto(self) -> MediaTrackerImportRunDto {
        MediaTrackerImportRunDto {
            id: self.id,
            status: self
                .status
                .parse()
                .unwrap_or(MediaTrackerImportStatus::Failed),
            fetched_count: self.fetched_count,
            matched_count: self.matched_count,
            updated_count: self.updated_count,
            deferred_count: self.deferred_count,
            skipped_count: self.skipped_count,
            error: self.error,
            created_at: self.created_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
        }
    }
}

async fn connection_dto(
    state: &AppState,
    tracker: db::UserMediaTracker,
) -> UserMediaTrackerDto {
    let provider = Addon::get(
        &state
            .ctx
            .db,
        tracker.addon_id,
    )
    .await
    .ok()
    .flatten()
    .map(|addon| addon.name)
    .unwrap_or_else(|| "Media tracker".into());
    UserMediaTrackerDto {
        id: tracker.id,
        addon_id: tracker.addon_id,
        provider,
        status: tracker
            .status
            .to_string(),
        remote_account_id: tracker.remote_account_id,
        remote_account_name: tracker.remote_account_name,
        last_success_at: tracker.last_success_at,
        last_error_at: tracker.last_error_at,
        last_error: tracker.last_error,
    }
}

#[get("/remux/media-trackers/providers")]
pub async fn get_media_tracker_providers(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let mut providers = Vec::new();
    for addon in Addon::list(
        &state
            .ctx
            .db,
    )
    .await?
    {
        let Some(provider) = state
            .ctx
            .addons
            .media_tracker_for(addon.id)
        else {
            continue;
        };
        providers.push(MediaTrackerProviderDto {
            addon_id: addon.id,
            kind: addon
                .preset
                .kind,
            name: addon.name,
            configured: state
                .ctx
                .config
                .trakt_client_id
                .as_ref()
                .is_some_and(|value| {
                    !value
                        .trim()
                        .is_empty()
                })
                && state
                    .ctx
                    .config
                    .trakt_client_secret
                    .as_ref()
                    .is_some_and(|value| {
                        !value
                            .expose()
                            .trim()
                            .is_empty()
                    }),
            history_import: provider
                .capabilities()
                .history_import,
        });
    }
    Ok(Json(providers))
}

#[get("/remux/users/{user_id}/media-trackers")]
pub async fn get_user_media_trackers(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .context_not_found("User not found")?;
    let mut result = Vec::new();
    for tracker in db::UserMediaTracker::list_for_user(
        &state
            .ctx
            .db,
        user_id,
    )
    .await?
    {
        result.push(connection_dto(&state, tracker).await);
    }
    Ok(Json(result))
}

#[post("/remux/users/{user_id}/media-trackers/{addon_id}/device-auth")]
pub async fn begin_media_tracker_device_auth(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path((user_id, addon_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .context_not_found("User not found")?;
    let provider = state
        .ctx
        .addons
        .media_tracker_for(addon_id)
        .context_not_found("Media tracker provider not found")?;
    let tctx = MediaTrackerCtx {
        config: std::sync::Arc::new(
            state
                .ctx
                .config
                .clone(),
        ),
    };
    let start = provider
        .begin_device_auth(&tctx)
        .await
        .map_err(anyhow::Error::from)
        .context_bad_request("Could not start Trakt authorization")?;
    let attempt_id = crate::common::get_uuid();
    let now = Utc::now().naive_utc();
    let interval_seconds = start
        .interval
        .as_secs()
        .max(1) as i64;
    sqlx::query(
        "INSERT INTO media_tracker_auth_attempts \
         (id, user_id, addon_id, poll_token, poll_interval_seconds, next_poll_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(attempt_id)
    .bind(user_id)
    .bind(addon_id)
    .bind(start.poll_token)
    .bind(interval_seconds)
    .bind(now + Duration::seconds(interval_seconds))
    .bind(now + Duration::seconds(start.expires_in.as_secs() as i64))
    .execute(&state.ctx.db)
    .await?;
    Ok(Json(MediaTrackerDeviceAuthDto {
        attempt_id,
        verification_url: start.verification_url,
        user_code: start.user_code,
        interval_seconds: interval_seconds as u64,
        expires_in_seconds: start
            .expires_in
            .as_secs(),
    }))
}

#[post(
    "/remux/users/{user_id}/media-trackers/{addon_id}/device-auth/{attempt_id}/poll"
)]
pub async fn poll_media_tracker_device_auth(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path((user_id, addon_id, attempt_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let attempt = sqlx::query_as::<_, AuthAttempt>(
        "SELECT id, user_id, addon_id, poll_token, poll_interval_seconds, next_poll_at, expires_at \
         FROM media_tracker_auth_attempts WHERE id = ?1 AND user_id = ?2 AND addon_id = ?3",
    )
    .bind(attempt_id)
    .bind(user_id)
    .bind(addon_id)
    .fetch_optional(&state.ctx.db)
    .await?
    .context_not_found("Authorization attempt not found")?;
    let now = Utc::now().naive_utc();
    if now >= attempt.expires_at {
        sqlx::query("DELETE FROM media_tracker_auth_attempts WHERE id = ?1")
            .bind(attempt.id)
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await?;
        return Ok(Json(MediaTrackerAuthPollDto {
            status: MediaTrackerAuthStatus::Expired,
            connection: None,
        }));
    }
    if now < attempt.next_poll_at {
        return Ok(Json(MediaTrackerAuthPollDto {
            status: MediaTrackerAuthStatus::Pending,
            connection: None,
        }));
    }
    sqlx::query(
        "UPDATE media_tracker_auth_attempts SET next_poll_at = ?2 WHERE id = ?1",
    )
    .bind(attempt.id)
    .bind(now + Duration::seconds(attempt.poll_interval_seconds))
    .execute(
        &state
            .ctx
            .db,
    )
    .await?;
    let provider = state
        .ctx
        .addons
        .media_tracker_for(addon_id)
        .context_not_found("Media tracker provider not found")?;
    let tctx = MediaTrackerCtx {
        config: std::sync::Arc::new(
            state
                .ctx
                .config
                .clone(),
        ),
    };
    match provider
        .poll_device_auth(&attempt.poll_token, &tctx)
        .await
        .map_err(anyhow::Error::from)
        .context_bad_request("Could not poll Trakt authorization")?
    {
        DeviceAuthPoll::Pending => Ok(Json(MediaTrackerAuthPollDto {
            status: MediaTrackerAuthStatus::Pending,
            connection: None,
        })),
        DeviceAuthPoll::SlowDown => {
            let slower_interval = attempt
                .poll_interval_seconds
                .saturating_add(5);
            sqlx::query(
                "UPDATE media_tracker_auth_attempts SET poll_interval_seconds = ?2, \
                 next_poll_at = ?3 WHERE id = ?1",
            )
            .bind(attempt.id)
            .bind(slower_interval)
            .bind(now + Duration::seconds(slower_interval))
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await?;
            Ok(Json(MediaTrackerAuthPollDto {
                status: MediaTrackerAuthStatus::Pending,
                connection: None,
            }))
        }
        DeviceAuthPoll::Denied => {
            sqlx::query("DELETE FROM media_tracker_auth_attempts WHERE id = ?1")
                .bind(attempt.id)
                .execute(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            Ok(Json(MediaTrackerAuthPollDto {
                status: MediaTrackerAuthStatus::Denied,
                connection: None,
            }))
        }
        DeviceAuthPoll::Expired => {
            sqlx::query("DELETE FROM media_tracker_auth_attempts WHERE id = ?1")
                .bind(attempt.id)
                .execute(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            Ok(Json(MediaTrackerAuthPollDto {
                status: MediaTrackerAuthStatus::Expired,
                connection: None,
            }))
        }
        DeviceAuthPoll::Approved(connection) => {
            let mut tracker = db::UserMediaTracker::new(
                user_id,
                addon_id,
                connection.credentials,
                provider
                    .capabilities()
                    .default_event_filter,
            );
            tracker.remote_account_id = connection.remote_account_id;
            tracker.remote_account_name = connection.remote_account_name;
            tracker
                .upsert(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            let tracker = db::UserMediaTracker::get_for_user_and_addon(
                &state
                    .ctx
                    .db,
                user_id,
                addon_id,
            )
            .await?
            .context_not_found("Connected tracker was not saved")?;
            sqlx::query("DELETE FROM media_tracker_auth_attempts WHERE id = ?1")
                .bind(attempt.id)
                .execute(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            Ok(Json(MediaTrackerAuthPollDto {
                status: MediaTrackerAuthStatus::Approved,
                connection: Some(connection_dto(&state, tracker).await),
            }))
        }
    }
}

async fn import_run(db: &sqlx::SqlitePool, run_id: Uuid) -> anyhow::Result<ImportRun> {
    sqlx::query_as::<_, ImportRun>(
        "SELECT id, status, fetched_count, matched_count, updated_count, deferred_count, \
         skipped_count, error, created_at, started_at, completed_at \
         FROM media_tracker_import_runs WHERE id = ?1",
    )
    .bind(run_id)
    .fetch_one(db)
    .await
    .map_err(Into::into)
}

#[post("/remux/users/{user_id}/media-trackers/{tracker_id}/imports")]
pub async fn start_media_tracker_import(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path((user_id, tracker_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let tracker = db::UserMediaTracker::get(
        &state
            .ctx
            .db,
        tracker_id,
    )
    .await?
    .filter(|tracker| tracker.user_id == user_id)
    .context_not_found("Media tracker connection not found")?;
    if let Some(existing) = sqlx::query_as::<_, ImportRun>(
        "SELECT id, status, fetched_count, matched_count, updated_count, deferred_count, \
         skipped_count, error, created_at, started_at, completed_at \
         FROM media_tracker_import_runs WHERE user_media_tracker_id = ?1 \
         AND status IN ('queued', 'running') ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tracker.id)
    .fetch_optional(&state.ctx.db)
    .await?
    {
        return Ok((StatusCode::ACCEPTED, Json(existing.dto())));
    }
    let run_id = crate::common::get_uuid();
    sqlx::query(
        "INSERT INTO media_tracker_import_runs (id, user_media_tracker_id, status) \
         VALUES (?1, ?2, 'queued')",
    )
    .bind(run_id)
    .bind(tracker.id)
    .execute(
        &state
            .ctx
            .db,
    )
    .await?;

    let ctx = state
        .ctx
        .clone();
    tokio::spawn(async move {
        let started = Utc::now().naive_utc();
        let _ = sqlx::query(
            "UPDATE media_tracker_import_runs SET status = 'running', started_at = ?2 WHERE id = ?1",
        )
        .bind(run_id)
        .bind(started)
        .execute(&ctx.db)
        .await;
        match crate::services::media_tracker::import_tracker_history(&ctx, tracker.id)
            .await
        {
            Ok(stats) => {
                let _ = sqlx::query(
                    "UPDATE media_tracker_import_runs SET status = 'succeeded', fetched_count = ?2, \
                     matched_count = ?3, updated_count = ?4, deferred_count = ?5, skipped_count = ?6, \
                     completed_at = ?7 WHERE id = ?1",
                )
                .bind(run_id)
                .bind(stats.fetched)
                .bind(stats.matched)
                .bind(stats.updated)
                .bind(stats.deferred)
                .bind(stats.skipped)
                .bind(Utc::now().naive_utc())
                .execute(&ctx.db)
                .await;
            }
            Err(error) => {
                let message = error.to_string();
                let _ = sqlx::query(
                    "UPDATE media_tracker_import_runs SET status = 'failed', error = ?2, completed_at = ?3 WHERE id = ?1",
                )
                .bind(run_id)
                .bind(&message)
                .bind(Utc::now().naive_utc())
                .execute(&ctx.db)
                .await;
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(
            import_run(
                &state
                    .ctx
                    .db,
                run_id,
            )
            .await?
            .dto(),
        ),
    ))
}

#[get("/remux/users/{user_id}/media-trackers/{tracker_id}/imports/{run_id}")]
pub async fn get_media_tracker_import(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path((user_id, tracker_id, run_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let allowed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_tracker_import_runs r \
         JOIN user_media_trackers t ON t.id = r.user_media_tracker_id \
         WHERE r.id = ?1 AND t.id = ?2 AND t.user_id = ?3",
    )
    .bind(run_id)
    .bind(tracker_id)
    .bind(user_id)
    .fetch_one(
        &state
            .ctx
            .db,
    )
    .await?;
    (allowed > 0)
        .then_some(())
        .context_not_found("Import run not found")?;
    Ok(Json(
        import_run(
            &state
                .ctx
                .db,
            run_id,
        )
        .await?
        .dto(),
    ))
}

#[delete("/remux/users/{user_id}/media-trackers/{tracker_id}")]
pub async fn delete_user_media_tracker(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path((user_id, tracker_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let tracker = db::UserMediaTracker::get(
        &state
            .ctx
            .db,
        tracker_id,
    )
    .await?
    .filter(|tracker| tracker.user_id == user_id)
    .context_not_found("Media tracker connection not found")?;
    if let Some(provider) = state
        .ctx
        .addons
        .media_tracker_for(tracker.addon_id)
    {
        let tctx = MediaTrackerCtx {
            config: std::sync::Arc::new(
                state
                    .ctx
                    .config
                    .clone(),
            ),
        };
        let _ = provider
            .disconnect(&tracker.credentials, &tctx)
            .await;
    }
    db::UserMediaTracker::delete(
        &state
            .ctx
            .db,
        tracker.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
