use std::collections::HashMap;

use anyhow::Context;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api,
    api::system::QuickConnectEntry,
    common::{get_uuid, server_id},
    db,
    db::{auth, user::User},
    services::MediaResolveService,
    ws::WsEvent,
};
use axum_anyhow::ApiResult as Result;
use remux_sdks::remux::Username;

use super::{
    items::{ItemsQueryResultBuilder, item, items, items_flat},
    mock_items,
    shows::livetv_view_item,
};

#[post("/users/{user_id}/configuration")]
pub async fn user_configuration_update(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(payload): Json<api::UserConfiguration>,
) -> Result<impl IntoResponse> {
    db::User::save_configuration(
        &state
            .ctx
            .db,
        &session
            .user
            .id,
        &payload,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Jellyfin SDK-compatible route: POST /Users/Configuration?userId=<id>
///
/// The URL-rewrite middleware lowercases all non-file paths, so the registered
/// route is `/users/configuration`.  Auth is via session (same as the canonical
/// `/users/{user_id}/configuration` route above).
#[query]
#[derive(Debug, Default)]
struct UserConfigurationQuery {
    user_id: Option<Uuid>,
}

#[post("/users/configuration")]
pub async fn user_configuration_legacy(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    _query: Query<UserConfigurationQuery>,
    Json(payload): Json<api::UserConfiguration>,
) -> Result<impl IntoResponse> {
    db::User::save_configuration(
        &state
            .ctx
            .db,
        &_session
            .user
            .id,
        &payload,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct DisplayPrefQuery {
    user_id: Option<Uuid>,
    client: String,
}

#[get("/displaypreferences/{id}")]
pub async fn get_display_preferences(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<String>,
    Query(q): Query<DisplayPrefQuery>,
) -> Result<impl IntoResponse> {
    let user = if let Some(user_id) = q.user_id {
        db::User::get_by_id(
            &state
                .ctx
                .db,
            &user_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("User not found"))?
    } else {
        session.user
    };

    let result = db::JellyfinDisplayPrefs::get_by_filter(
        &state
            .ctx
            .db,
        &db::JellyfinDisplayPrefsFilter {
            id: Some(vec![id]),
            client: Some(
                q.client
                    .clone(),
            ),
            user_id: Some(user.id),
            ..Default::default()
        },
    )
    .await?;

    let mut prefs = if let Some(record) = result
        .records
        .first()
    {
        record.clone()
    } else {
        db::JellyfinDisplayPrefs {
            client: Some(q.client),
            ..Default::default()
        }
    };

    if !prefs
        .data
        .custom_prefs
        .keys()
        .any(|k| k.starts_with("homesection"))
    {
        prefs
            .data
            .custom_prefs
            .extend(db::default_homescreen_custom_prefs());
    }

    Ok(Json(api::db_display_prefs_to_dto(prefs)))
}

#[post("/displaypreferences/{id}")]
pub async fn update_display_preferences(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<String>,
    Query(q): Query<DisplayPrefQuery>,
    Json(payload): Json<api::DisplayPreferencesDto>,
) -> Result<impl IntoResponse> {
    let user = if let Some(user_id) = q.user_id {
        db::User::get_by_id(
            &state
                .ctx
                .db,
            &user_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("User not found"))?
    } else {
        session.user
    };

    let prefs = db::JellyfinDisplayPrefs {
        id: id.clone(),
        user_id: user.id,
        client: Some(
            q.client
                .clone(),
        ),
        data: sqlx::types::Json(db::JellyfinDisplayPrefsData::from(payload)),
    };

    prefs
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

fn require_self_or_admin(target_id: Uuid, session: &auth::AuthSession) -> Result<()> {
    if target_id
        != session
            .user
            .id
        && !session
            .user
            .is_admin
    {
        return Err(anyhow::anyhow!("Forbidden").context_unauthorized("forbidden"));
    }
    Ok(())
}

fn build_auth_response(
    data_dir: &std::path::Path,
    device: auth::Device,
    user: db::User,
) -> Json<api::AuthenticationResult> {
    let session_info = api::SessionInfoDto {
        id: Some(
            device
                .id
                .clone(),
        ),
        device_id: Some(
            device
                .id
                .clone(),
        ),
        device_name: Some(
            device
                .name
                .clone(),
        ),
        client: Some(
            device
                .app_name
                .clone(),
        ),
        application_version: Some(
            device
                .app_version
                .clone(),
        ),
        user_id: device
            .user_id
            .to_string(),
        user_name: Some(
            user.username
                .clone(),
        ),
        server_id: server_id(),
        is_active: true,
        play_state: Some(api::PlayerStateInfo::default()),
        capabilities: Some(api::ClientCapabilitiesDto {
            supports_persistent_identifier: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let now = chrono::Utc::now();
    let mut user_dto = api::db_user_to_dto(data_dir, user);
    user_dto.last_login_date = Some(now);
    user_dto.last_activity_date = Some(now);

    Json(api::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        session_info: Some(session_info),
        user: Some(user_dto),
    })
}

#[post("/users/authenticatebyname")]
pub async fn users_authenticatebyname(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
    Json(data): Json<api::AuthenticateUserByName>,
) -> Result<impl IntoResponse> {
    let user = User::authenticate(
        &state
            .ctx
            .db,
        data.username
            .as_deref()
            .unwrap_or(""),
        data.pw
            .as_deref()
            .unwrap_or(""),
    )
    .await?
    .context_unauthorized("not found")?;
    let device = auth::Device::new_from_header(auth_header, &user)?;
    device
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

    Ok(build_auth_response(
        &state
            .ctx
            .config
            .data_dir,
        device,
        user,
    ))
}

#[post("/users/authenticatewithquickconnect")]
pub async fn authenticate_with_quickconnect(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
    Json(body): Json<api::AuthenticateWithQuickConnect>,
) -> Result<impl IntoResponse> {
    let entry = state
        .ctx
        .store
        .get::<QuickConnectEntry>(format!("qc:{}", body.secret))
        .context_unauthorized("QuickConnect request not found or expired")?;

    if !entry.authenticated {
        return Err(anyhow::anyhow!("not authenticated"))
            .context_unauthorized("QuickConnect request has not been approved yet");
    }

    let user_id = entry
        .user_id
        .context_unauthorized("QuickConnect entry missing user")?;

    let user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .context_unauthorized("User not found")?;

    let device = auth::Device {
        id: auth_header
            .device_id
            .unwrap_or_else(|| get_uuid().to_string()),
        name: auth_header
            .device
            .unwrap_or_else(|| "QuickConnect".to_string()),
        app_name: auth_header
            .client
            .unwrap_or_else(|| "QuickConnect".to_string()),
        app_version: auth_header
            .version
            .unwrap_or_else(|| "1.0".to_string()),
        user_id: user.id,
        access_token: get_uuid().to_string(),
        last_activity_at: None,
        capabilities: None,
        remote_ip: None,
    };
    device
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

    // clean up store entries
    state
        .ctx
        .store
        .delete(format!("qc:{}", body.secret));
    state
        .ctx
        .store
        .delete(format!("qc:code:{}", entry.code));

    Ok(build_auth_response(
        &state
            .ctx
            .config
            .data_dir,
        device,
        user,
    ))
}

#[get("/users")]
pub async fn users(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let items = db::User::get_by_filter(
        &state
            .ctx
            .db,
        &db::UserFilter {
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|x| {
        let mut item = api::db_user_to_dto(
            &state
                .ctx
                .config
                .data_dir,
            x,
        );
        //item.type_ = api::MediaType::CollectionFolder;
        //item.collection_type = Some(api::CollectionType::Movies);
        item
    })
    .collect::<Vec<api::UserDto>>();

    Ok(Json(items))
}

#[get("/users/me")]
pub async fn users_me(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::db_user_to_dto(
        &state
            .ctx
            .config
            .data_dir,
        session.user,
    ))
    .into_response())
}

#[post("/users/{user_id}/favoriteitems/{id}")]
pub async fn mark_favorite(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context("not found")?;
    let ms = media
        .mark_favorite(
            &state
                .ctx
                .db,
            &session.user,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

#[delete("/users/{user_id}/favoriteitems/{id}")]
pub async fn unmark_favorite(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context("not found")?;
    let ms = media
        .unmark_favorite(
            &state
                .ctx
                .db,
            &session.user,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

#[post("/userfavoriteitems/{id}")]
pub async fn mark_favorite_modern(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context_not_found("Item not found")?;
    let s = media
        .mark_favorite(
            &state
                .ctx
                .db,
            &session.user,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(s, &media)).into_response())
}

#[delete("/userfavoriteitems/{id}")]
pub async fn unmark_favorite_modern(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context_not_found("Item not found")?;
    let s = media
        .unmark_favorite(
            &state
                .ctx
                .db,
            &session.user,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(s, &media)).into_response())
}

#[post("/users/{user_id}/playeditems/{id}")]
pub async fn mark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context("not found")?;
    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let ms = media
        .mark_played(
            &state
                .ctx
                .db,
            &session.user,
            true,
            server_config.release_date_threshold(),
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

#[delete("/users/{user_id}/playeditems/{id}")]
pub async fn unmark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context("not found")?;
    let ms = media
        .mark_unplayed(
            &state
                .ctx
                .db,
            &session.user,
            true,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

#[get("/users/{user_id}/groupingoptions")]
pub async fn users_groupingoptions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json::<Vec<api::SpecialViewOptionDto>>(vec![]))
}

#[post("/users/new")]
pub async fn create_user(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<api::CreateUserByName>,
) -> Result<impl IntoResponse> {
    let password = payload
        .password
        .as_deref()
        .unwrap_or("");
    let mut user = User::new_with_password(
        String::new(),
        payload
            .name
            .into_inner(),
        password,
        None,
    )?;
    user.save(
        &state
            .ctx
            .db,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(WsEvent::UserUpdated(user.id));
    Ok((
        StatusCode::OK,
        Json(api::db_user_to_dto(
            &state
                .ctx
                .config
                .data_dir,
            user,
        )),
    )
        .into_response())
}

#[delete("/users/{user_id}")]
pub async fn delete_user(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if user_id
        == session
            .user
            .id
    {
        return Err(anyhow::anyhow!("Cannot delete yourself")
            .context_bad_request("cannot delete own account"));
    }
    db::User::delete(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(WsEvent::UserDeleted(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/password")]
pub async fn change_password(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<api::UpdateUserPassword>,
) -> Result<impl IntoResponse> {
    require_self_or_admin(user_id, &session)?;

    let mut user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("User not found"))?;

    if user_id
        == session
            .user
            .id
        && !session
            .user
            .is_admin
    {
        let current = payload
            .current_pw
            .as_deref()
            .unwrap_or("");
        if !user.verify_password(current)? {
            return Err(anyhow::anyhow!("Current password is incorrect")
                .context_unauthorized("invalid password"));
        }
    }

    let new_pw = payload
        .new_pw
        .as_deref()
        .unwrap_or("");
    user.set_password(new_pw)?;
    user.save(
        &state
            .ctx
            .db,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/policy")]
pub async fn update_user_policy(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
    Json(policy): Json<api::UserPolicy>,
) -> Result<impl IntoResponse> {
    let mut user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("User not found"))?;
    user.is_admin = policy.is_administrator;
    user.policy = Some(sqlx::types::Json(policy));
    user.save(
        &state
            .ctx
            .db,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}")]
pub async fn update_user(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<api::UserDto>,
) -> Result<impl IntoResponse> {
    require_self_or_admin(user_id, &session)?;
    let mut user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("User not found"))?;
    let username = Username::try_new(payload.name)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context_bad_request("Invalid username")?;
    user.username = username.into_inner();
    if let Some(config) = payload.configuration {
        user.configuration = Some(sqlx::types::Json(config));
    }
    user.save(
        &state
            .ctx
            .db,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ===== Route aliases (same handler, different path) =====

#[get("/users/public")]
pub async fn users_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let all_users = sqlx::query_as::<_, User>("SELECT * FROM users")
        .fetch_all(&state.ctx.db)
        .await?;

    let dtos: Vec<api::UserDto> = all_users
        .into_iter()
        .filter(|u| {
            let policy = u.policy.as_ref().map(|p| &p.0);
            let is_hidden = policy.map(|p| p.is_hidden).unwrap_or(false);
            !is_hidden
        })
        .map(|u| api::db_user_to_dto(&state.ctx.config.data_dir, u))
        .collect();

    Ok(Json(dtos).into_response())
}

#[get("/users/{user_id}")]
pub async fn users_get_by_id(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if user_id
        == session
            .user
            .id
    {
        return Ok(Json(api::db_user_to_dto(
            &state
                .ctx
                .config
                .data_dir,
            session.user,
        ))
        .into_response());
    }
    if !session
        .user
        .is_admin
    {
        return Err(anyhow::anyhow!("Forbidden").context_unauthorized("forbidden"));
    }
    let user = db::User::get_by_id(
        &state
            .ctx
            .db,
        &user_id,
    )
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!("User not found").context_not_found("user not found")
    })?;
    Ok(Json(api::db_user_to_dto(
        &state
            .ctx
            .config
            .data_dir,
        user,
    ))
    .into_response())
}

#[get("/users/{user_id}/items/{id}")]
pub async fn users_items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    return Ok(Json(
        item(
            state,
            session,
            id,
            q.fields
                .as_deref(),
        )
        .await?,
    )
    .into_response());
}

#[get("/users/{user_id}/items")]
pub async fn users_items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    items(State(state), session, Query(q)).await
}

#[get("/users/{user_id}/items/latest")]
pub async fn users_items_latest(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    items_flat(State(state), session, Query(q)).await
}

#[get("/userviews")]
pub async fn userviews(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let library_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
        promoted: Some(true),
        include_child_count: true,
        sort_by: vec![api::ItemSortBy::DisplayOrder],
        sort_order: vec![api::SortOrder::Ascending],
        ..Default::default()
    };
    let channel_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvChannel]),
        enabled: Some(true),
        ..Default::default()
    };
    let (library_result, channel_result) = tokio::join!(
        db::Media::get_by_filter(
            &state
                .ctx
                .db,
            &library_filter
        ),
        db::Media::get_by_filter(
            &state
                .ctx
                .db,
            &channel_filter
        ),
    );

    let mut libraries = library_result?.records;

    // Hide libraries/collections that contain zero items visible to this user.
    // Hide libraries/collections that contain zero items visible to this user.
    if let Some(pf) = session
        .user
        .policy
        .as_ref()
        .and_then(|p| {
            p.filter_rules
                .as_ref()
        })
        .filter(|pf| {
            pf.groups
                .iter()
                .any(|g| {
                    !g.rules
                        .is_empty()
                })
        })
    {
        let mut to_remove = Vec::new();

        for library in &libraries {
            let mut qb = sqlx::QueryBuilder::new("SELECT 1 FROM media WHERE 1=1");

            // base restriction (same as before)
            qb.push(
                " AND kind NOT IN ('collection', 'folder', 'playlist', 'tv_channel')",
            );

            // apply user rules
            db::apply_filter_rules(&mut qb, pf);

            // limit to 1 match (we only care if it exists)
            qb.push(" LIMIT 1");

            let exists = qb
                .build()
                .fetch_optional(
                    &state
                        .ctx
                        .db,
                )
                .await
                .map(|row| row.is_some())
                .unwrap_or(false);

            if !exists {
                to_remove.push(library.id);
            }
        }

        libraries.retain(|m| !to_remove.contains(&m.id));
    }

    let mut items = libraries
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect::<Vec<api::BaseItemDto>>();

    // Inject a synthetic Live TV view if any enabled channels exist
    if !channel_result?
        .records
        .is_empty()
    {
        items.push(livetv_view_item());
    }

    let count = items.len() as i64;
    let result = ItemsQueryResultBuilder::with_dtos(session, items, count)
        .with_client_patches()
        .build();
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result.items,
        total_record_count: result.total_count,
        ..Default::default()
    }))
}

#[get("/userviews/groupingoptions")]
pub async fn userviews_groupingoptions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
        promoted: Some(true),
        ..Default::default()
    };
    let items = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &filter,
    )
    .await?
    .records
    .into_iter()
    .map(|m| remux_sdks::remux::SpecialViewOptionDto {
        name: Some(
            m.title
                .clone(),
        ),
        id: Some(m.id.to_string()),
    })
    .collect::<Vec<_>>();

    Ok(Json(items))
}

#[get("/users/{user_id}/views")]
pub async fn users_views(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    userviews(State(state), session).await
}

async fn resume_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
) -> Result<impl IntoResponse> {
    q.user_id = Some(
        session
            .user
            .id,
    );
    q.filters = Some(vec![api::ItemFilter::IsResumable]);
    if q.limit
        .is_none()
    {
        q.limit = Some(50);
    }
    if q.sort_by
        .is_none()
    {
        q.sort_by = Some(vec![api::ItemSortBy::DatePlayed]);
    }
    if q.sort_order
        .is_none()
    {
        q.sort_order = Some(vec![api::SortOrder::Descending]);
    }
    let server_config = crate::db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let result = db::Media::get_by_jellyfin_filter(
        &state
            .ctx
            .db,
        &q,
        true,
        Some(&session.user),
        Some(&server_config),
        None,
        None,
    )
    .await?;
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result
            .records
            .into_iter()
            .map(|m| api::db_media_to_item(m, false))
            .collect(),
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/users/{user_id}/items/resume")]
pub async fn users_items_resume(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    resume_items(state, session, q).await
}

#[get("/users/{user_id}/items/similar")]
pub async fn users_items_similar(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/users/{user_id}/intros")]
pub async fn users_intros(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

#[get("/users/{user_id}/items/{id}/intros")]
pub async fn users_items_intros(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((_user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    crate::api::intro::get_intros_inner(state, session, id).await
}

#[get("/useritems/resume")]
pub async fn useritems_resume(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    resume_items(state, session, q).await
}

#[post("/users/forgotpassword")]
pub async fn forgot_password() -> impl IntoResponse {
    Json(serde_json::json!({
        "Action": "ContactAdmin",
        "PinFile": null,
        "PinExpirationDate": null,
    }))
}

// ===== User avatar endpoints =====

fn avatar_path(data_dir: &std::path::Path, user_id: &Uuid) -> std::path::PathBuf {
    data_dir
        .join("meta")
        .join("avatars")
        .join(user_id.to_string())
}

pub fn user_has_avatar(data_dir: &std::path::Path, user_id: &Uuid) -> bool {
    avatar_path(data_dir, user_id).exists()
}

async fn upload_avatar_for(
    data_dir: &std::path::Path,
    user_id: &Uuid,
    image: crate::api::image::JellyfinImage,
) -> anyhow::Result<()> {
    let path = avatar_path(data_dir, user_id);
    tokio::fs::create_dir_all(
        path.parent()
            .unwrap(),
    )
    .await
    .context("failed to create avatars directory")?;
    tokio::fs::write(&path, &image.bytes)
        .await
        .context("failed to write avatar file")?;
    Ok(())
}

async fn delete_avatar_for(
    data_dir: &std::path::Path,
    user_id: &Uuid,
) -> anyhow::Result<()> {
    let path = avatar_path(data_dir, user_id);
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .context("failed to delete avatar file")?;
    }
    Ok(())
}

async fn serve_avatar_for(
    data_dir: std::path::PathBuf,
    user_id: Uuid,
) -> Result<impl IntoResponse> {
    let path = avatar_path(&data_dir, &user_id);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| {
            anyhow::anyhow!("avatar not found").context_not_found("avatar not found")
        })?;
    let content_type = crate::api::image::detect_content_type(&bytes);
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

// --- GET (no auth required — matches Jellyfin behaviour) ---

#[derive(Deserialize)]
struct UserImageQuery {
    #[serde(rename = "userId", alias = "user_id")]
    user_id: Option<Uuid>,
    tag: Option<String>,
}

#[get("/userimage")]
pub async fn get_user_image(
    State(state): State<AppState>,
    Query(q): Query<UserImageQuery>,
) -> Result<impl IntoResponse> {
    let uid = q
        .user_id
        .or_else(|| {
            q.tag
                .as_deref()
                .and_then(|t| Uuid::parse_str(t).ok())
        })
        .context_bad_request("userId required")?;
    serve_avatar_for(
        state
            .ctx
            .config
            .data_dir
            .clone(),
        uid,
    )
    .await
}

#[get("/users/{user_id}/images/{image_type}")]
pub async fn get_user_image_by_id(
    State(state): State<AppState>,
    Path((user_id, _image_type)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    serve_avatar_for(
        state
            .ctx
            .config
            .data_dir
            .clone(),
        user_id,
    )
    .await
}

#[get("/users/{user_id}/images/{image_type}/{index}")]
pub async fn get_user_image_by_id_indexed(
    State(state): State<AppState>,
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
) -> Result<impl IntoResponse> {
    serve_avatar_for(
        state
            .ctx
            .config
            .data_dir
            .clone(),
        user_id,
    )
    .await
}

// --- POST (upload) ---

#[post("/userimage")]
pub async fn upload_user_image(
    State(state): State<AppState>,
    session: auth::AuthSession,
    image: crate::api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    upload_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &session
            .user
            .id,
        image,
    )
    .await
    .context_internal("failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/images/{image_type}")]
pub async fn upload_user_image_legacy(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type)): Path<(Uuid, String)>,
    image: crate::api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    upload_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &user_id,
        image,
    )
    .await
    .context_internal("failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/images/{image_type}/{index}")]
pub async fn upload_user_image_indexed(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
    image: crate::api::image::JellyfinImage,
) -> Result<impl IntoResponse> {
    upload_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &user_id,
        image,
    )
    .await
    .context_internal("failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- DELETE ---

#[delete("/userimage")]
pub async fn delete_user_image(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    delete_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &session
            .user
            .id,
    )
    .await
    .context_internal("failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[delete("/users/{user_id}/images/{image_type}")]
pub async fn delete_user_image_legacy(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    delete_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &user_id,
    )
    .await
    .context_internal("failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[delete("/users/{user_id}/images/{image_type}/{index}")]
pub async fn delete_user_image_indexed(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
) -> Result<impl IntoResponse> {
    delete_avatar_for(
        &state
            .ctx
            .config
            .data_dir,
        &user_id,
    )
    .await
    .context_internal("failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ── Auth providers (stubs) ──────────────────────────────────────────────────

#[get("/auth/providers")]
pub async fn get_auth_providers(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<serde_json::Value>::new()))
}

#[get("/auth/passwordresetproviders")]
pub async fn get_password_reset_providers(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<serde_json::Value>::new()))
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server, insert_test_source,
        new_test_server,
    };
    use http::header::HeaderValue;
    use serde_json::json;

    #[tokio::test]
    async fn test_authenticate_valid_credentials() {
        let (server, _ctx) = new_test_server()
            .await
            .unwrap();

        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["AccessToken"]
                .as_str()
                .is_some_and(|t| !t.is_empty())
        );
        assert_eq!(body["User"]["Name"], "test");
    }

    #[tokio::test]
    async fn test_authenticate_wrong_password() {
        let (server, _ctx) = new_test_server()
            .await
            .unwrap();

        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "test", "Pw": "wrongpassword" }))
            .expect_failure()
            .await;

        resp.assert_status_unauthorized();
    }

    #[tokio::test]
    async fn test_authenticate_unknown_user() {
        let (server, _ctx) = new_test_server()
            .await
            .unwrap();

        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({ "Username": "nobody", "Pw": "test" }))
            .expect_failure()
            .await;

        resp.assert_status_unauthorized();
    }

    #[tokio::test]
    async fn test_update_display_preferences() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // POST to save display preferences
        let resp = server
            .post("/displaypreferences/usersettings")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("userId", ""), ("client", "emby")])
            .json(&json!({
                "Id": "usersettings",
                "SortBy": "SortName",
                "RememberIndexing": false,
                "PrimaryImageHeight": 250,
                "PrimaryImageWidth": 250,
                "ScrollDirection": "Horizontal",
                "ShowBackdrop": true,
                "RememberSorting": false,
                "SortOrder": "Ascending",
                "ShowSidebar": false,
                "Client": "emby",
                "CustomPrefs": {
                    "chromecastVersion": "stable",
                    "skipForwardLength": "30000",
                    "skipBackLength": "10000",
                    "enableNextVideoInfoOverlay": "True",
                    "tvhome": "",
                    "dashboardTheme": ""
                }
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);

        // GET to verify the saved preferences are returned
        let resp = server
            .get("/displaypreferences/usersettings")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("userId", ""), ("client", "emby")])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert_eq!(body["SortBy"], "SortName");
        assert_eq!(body["ShowBackdrop"], true);
        assert_eq!(body["ScrollDirection"], "Horizontal");
        assert_eq!(body["SortOrder"], "Ascending");
        assert_eq!(body["CustomPrefs"]["chromecastVersion"], "stable");
    }

    #[tokio::test]
    async fn test_update_user_configuration() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Get user ID from /users/me
        let resp = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        let user_id = user["Id"]
            .as_str()
            .unwrap();

        // POST user configuration
        let resp = server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "PlayDefaultAudioTrack": true,
                "SubtitleLanguagePreference": "eng",
                "DisplayMissingEpisodes": false,
                "SubtitleMode": "Default",
                "EnableLocalPassword": false,
                "HidePlayedInLatest": true,
                "RememberAudioSelections": true,
                "RememberSubtitleSelections": true,
                "EnableNextEpisodeAutoPlay": true,
                "DisplayCollectionsView": false
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);

        // GET user again to verify configuration was persisted
        let resp = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        assert_eq!(user["Configuration"]["SubtitleLanguagePreference"], "eng");
        assert_eq!(user["Configuration"]["EnableNextEpisodeAutoPlay"], true);
        assert_eq!(user["Configuration"]["HidePlayedInLatest"], true);
    }

    #[tokio::test]
    async fn test_update_user_configuration_jellyfin_sdk_route() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        let user_id = user["Id"]
            .as_str()
            .unwrap();

        // POST via the Jellyfin SDK-compatible route with userId query param
        let resp = server
            .post("/users/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("userId", user_id)])
            .json(&json!({
                "PlayDefaultAudioTrack": true,
                "SubtitleLanguagePreference": "fre",
                "DisplayMissingEpisodes": false,
                "SubtitleMode": "Default",
                "EnableLocalPassword": false,
                "HidePlayedInLatest": false,
                "RememberAudioSelections": true,
                "RememberSubtitleSelections": true,
                "EnableNextEpisodeAutoPlay": false,
                "DisplayCollectionsView": true
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);

        // Verify configuration was persisted
        let resp = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        assert_eq!(user["Configuration"]["SubtitleLanguagePreference"], "fre");
        assert_eq!(user["Configuration"]["DisplayCollectionsView"], true);
    }

    /// Continue Watching must return items ordered most-recently-played first (issue #19).
    #[tokio::test]
    async fn test_resume_items_ordered_by_last_played_at() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Create two distinct media items.
        let older = insert_test_source(&ctx.0).await;
        let newer = insert_test_source(&ctx.0).await;

        // Resolve the test user.
        let user = db::User::get_by_username(
            &ctx.0
                .db,
            "test",
        )
        .await
        .unwrap()
        .unwrap();

        // Insert user_media_state rows with explicit last_played_at so the
        // ordering is deterministic regardless of wall-clock speed.
        sqlx::query(
            "INSERT INTO user_media_state (user_id, media_id, playback_position, last_played_at) \
             VALUES (?1, ?2, 60, '2026-01-01T10:00:00Z'), (?1, ?3, 60, '2026-01-01T11:00:00Z')",
        )
        .bind(user.id)
        .bind(older.id)
        .bind(newer.id)
        .execute(&ctx.0.db)
        .await
        .unwrap();

        let resp = server
            .get("/users/me/items/resume")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), 2, "both in-progress items must appear");
        // newer (11:00) must be first, older (10:00) second
        assert_eq!(
            items[0]["Id"]
                .as_str()
                .unwrap(),
            newer
                .id
                .to_string(),
            "most-recently-played item must be first"
        );
        assert_eq!(
            items[1]["Id"]
                .as_str()
                .unwrap(),
            older
                .id
                .to_string(),
            "least-recently-played item must be second"
        );
    }

    /// Marking a season as played must not mark unreleased episodes when the
    /// release-date filter is enabled.
    #[tokio::test]
    async fn mark_season_played_skips_unreleased_episodes() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);

        let series_imdb =
            db::NonEmptyString::try_new("tt_msp_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "TestSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        let make_ep = |n: u32, released_at: Option<chrono::NaiveDateTime>| db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(n.into()),
            }),
            title: format!("Ep{n}"),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(n as i64),
            digital_released_at: released_at,
            ..Default::default()
        };

        let mut ep1 = make_ep(1, Some(now - chrono::Duration::days(7)));
        ep1.save(db)
            .await
            .unwrap();
        let mut ep2 = make_ep(2, Some(future));
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark the season as played via the API.
        server
            .post(&format!("/users/{}/playeditems/{}", user.id, season.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // ep1 (released) must be marked played.
        let ep1_state: Option<db::UserMediaState> = sqlx::query_as(
            "SELECT * FROM user_media_state WHERE user_id = ? AND media_id = ?",
        )
        .bind(user.id)
        .bind(ep1.id)
        .fetch_optional(db)
        .await
        .unwrap();
        assert!(
            ep1_state
                .map(|s| s.play_count > 0)
                .unwrap_or(false),
            "released episode must be marked played"
        );

        // ep2 (unreleased) must NOT be marked played.
        let ep2_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(ep2.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(ep2_count, 0, "unreleased episode must not be marked played");
    }

    /// When all released episodes in a season are individually marked played,
    /// the season itself should cascade to played — even if an unreleased episode
    /// exists (it is excluded from the "unplayed count" check).
    #[tokio::test]
    async fn mark_episode_played_cascades_season_when_all_released_watched() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);

        let series_imdb =
            db::NonEmptyString::try_new("tt_mec_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "CascadeSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        let make_ep = |n: u32, released_at: Option<chrono::NaiveDateTime>| db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(n.into()),
            }),
            title: format!("Ep{n}"),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(n as i64),
            digital_released_at: released_at,
            ..Default::default()
        };

        let mut ep1 = make_ep(1, Some(now - chrono::Duration::days(7)));
        ep1.save(db)
            .await
            .unwrap();
        let mut ep2 = make_ep(2, Some(future));
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark only ep1 (the single released episode) as played.
        server
            .post(&format!("/users/{}/playeditems/{}", user.id, ep1.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // The season should cascade to played because all released episodes are watched.
        let season_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(season.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(
            season_count, 1,
            "season should be marked played when all released episodes are watched"
        );

        // ep2 (unreleased) must still be unplayed.
        let ep2_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(ep2.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(ep2_count, 0, "unreleased episode must remain unplayed");
    }

    /// Marking a whole series as played must not mark seasons that have no released
    /// episodes (e.g. a future season). Verified bug: `child_season_ids` had no threshold filter.
    #[tokio::test]
    async fn mark_series_played_skips_unreleased_seasons() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);
        let past = now - chrono::Duration::days(7);

        let series_imdb =
            db::NonEmptyString::try_new("tt_mss_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "SkipUnreleasedSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        // Season 1 — has a released episode
        let mut s1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        s1.save(db)
            .await
            .unwrap();

        let mut ep1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(1),
            }),
            title: "S1E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s1.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(past),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // Season 2 — only has an unreleased episode
        let mut s2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(2),
                episode: None,
            }),
            title: "Season 2".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(2),
            ..Default::default()
        };
        s2.save(db)
            .await
            .unwrap();

        let mut ep2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(2),
                episode: Some(1),
            }),
            title: "S2E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s2.id),
            parent_idx: Some(2),
            idx: Some(1),
            digital_released_at: Some(future),
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        server
            .post(&format!("/users/{}/playeditems/{}", user.id, series.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // Season 1 must be marked played.
        let s1_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(s1.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert!(s1_count > 0, "released season must be marked played");

        // Season 2 must NOT be marked played.
        let s2_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(s2.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(s2_count, 0, "unreleased season must not be marked played");

        // ep2 (unreleased) must NOT be marked played.
        let ep2_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(ep2.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(ep2_count, 0, "unreleased episode must not be marked played");
    }

    /// After marking a whole series played, the series itself should show
    /// `unplayed_item_count = 0` when the release-date filter is active (unreleased
    /// episodes must not count toward the badge). Verified bug: the count query had
    /// no threshold filter.
    #[tokio::test]
    async fn mark_series_played_series_shows_as_watched() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);
        let past = now - chrono::Duration::days(7);

        let series_imdb =
            db::NonEmptyString::try_new("tt_msw_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "SeriesWatchedTest".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            digital_released_at: Some(past),
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut s1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        s1.save(db)
            .await
            .unwrap();

        let mut ep1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(1),
            }),
            title: "S1E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s1.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(past),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // Unreleased episode in the same season
        let mut ep2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(2),
            }),
            title: "S1E2".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s1.id),
            parent_idx: Some(1),
            idx: Some(2),
            digital_released_at: Some(future),
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        server
            .post(&format!("/users/{}/playeditems/{}", user.id, series.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // Fetch the series via get_by_filter with user state and release threshold.
        let threshold = cfg
            .release_date_threshold()
            .unwrap();
        let results = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                id: Some(vec![series.id]),
                include_user_state: true,
                user_id: Some(user.id),
                digital_released_before: Some(threshold),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let series_record = results
            .records
            .into_iter()
            .find(|m| m.id == series.id)
            .expect("series must be in result");

        assert_eq!(
            series_record.unplayed_item_count,
            Some(0),
            "unplayed_item_count must be 0 when unreleased episodes are excluded by threshold"
        );

        let played_at = series_record
            .user_state
            .as_ref()
            .and_then(|s| s.played_at);
        assert!(played_at.is_some(), "series must have played_at set");
    }

    /// Marking a season played should cascade to the series when all RELEASED seasons
    /// are watched — even if an unreleased season exists.
    /// Bug: `cascade_played_to_series` used `count_unplayed_children` with no threshold,
    /// so the unreleased season was counted as unplayed and cascade was suppressed.
    #[tokio::test]
    async fn mark_season_played_cascades_series_when_all_released_seasons_watched() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let future = now + chrono::Duration::days(30);
        let past = now - chrono::Duration::days(7);

        let series_imdb =
            db::NonEmptyString::try_new("tt_msc_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "CascadeSeriesTest".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        // Season 1 — has a released episode
        let mut s1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        s1.save(db)
            .await
            .unwrap();

        let mut ep1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(1),
            }),
            title: "S1E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s1.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(past),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // Season 2 — only has an unreleased episode (upcoming season)
        let mut s2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(2),
                episode: None,
            }),
            title: "Season 2".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(2),
            ..Default::default()
        };
        s2.save(db)
            .await
            .unwrap();

        let mut ep2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(2),
                episode: Some(1),
            }),
            title: "S2E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(s2.id),
            parent_idx: Some(2),
            idx: Some(1),
            digital_released_at: Some(future),
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark only Season 1 as played (not the series directly).
        server
            .post(&format!("/users/{}/playeditems/{}", user.id, s1.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // The series must cascade to played because all released seasons are watched.
        let series_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(series.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert!(
            series_count > 0,
            "series must cascade to played when all released seasons are watched"
        );

        // Season 2 (unreleased) must remain unplayed.
        let s2_count: i64 =
            sqlx::query_scalar("SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?")
                .bind(user.id)
                .bind(s2.id)
                .fetch_optional(db)
                .await
                .unwrap()
                .unwrap_or(0);
        assert_eq!(s2_count, 0, "unreleased season must remain unplayed");
    }

    /// When a season is marked played with the filter active, an episode whose
    /// `digital_released_at` AND `released_at` are both NULL (anime, no TVDB air date)
    /// must NOT be marked played. Currently fails because the NULL date falls back to
    /// `'1900-01-01'` in `push_release_date_filter`, treating the episode as released.
    #[tokio::test]
    async fn null_air_date_episode_not_marked_played_when_season_marked() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();

        let series_imdb =
            db::NonEmptyString::try_new("tt_null_s_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "NullDateSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        let make_ep =
            |n: u32, digital_released_at: Option<chrono::NaiveDateTime>| db::Media {
                id: uuid::Uuid::from(&db::MediaIdRaw {
                    kind: db::MediaKind::Episode,
                    external_ids: db::ExternalIds {
                        series_imdb: Some(series_imdb.clone()),
                        ..Default::default()
                    },
                    season: Some(1),
                    episode: Some(n.into()),
                }),
                title: format!("Ep{n}"),
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                grandparent_id: Some(series.id),
                parent_id: Some(season.id),
                parent_idx: Some(1),
                idx: Some(n as i64),
                digital_released_at,
                ..Default::default()
            };

        // ep1: released (past date)
        let mut ep1 = make_ep(1, Some(now - chrono::Duration::days(7)));
        ep1.save(db)
            .await
            .unwrap();

        // ep2: no air date at all — anime series where TVDB has no release date
        let mut ep2 = make_ep(2, None);
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        server
            .post(&format!("/users/{}/playeditems/{}", user.id, season.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        let ep1_count: i64 = sqlx::query_scalar(
            "SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?",
        )
        .bind(user.id)
        .bind(ep1.id)
        .fetch_optional(db)
        .await
        .unwrap()
        .unwrap_or(0);
        assert!(ep1_count > 0, "released episode must be marked played");

        // ep2 has no air date — must be treated as unreleased and stay unplayed.
        let ep2_count: i64 = sqlx::query_scalar(
            "SELECT COALESCE(play_count, 0) FROM user_media_state WHERE user_id = ? AND media_id = ?",
        )
        .bind(user.id)
        .bind(ep2.id)
        .fetch_optional(db)
        .await
        .unwrap()
        .unwrap_or(0);
        assert_eq!(
            ep2_count, 0,
            "null-date episode (no TVDB air date) must not be marked played"
        );
    }

    /// With the release-date filter active, an episode with no air date (NULL
    /// `digital_released_at` and `released_at`) must not contribute to the series'
    /// `unplayed_item_count`. Currently fails because the NULL date collapses to
    /// `'1900-01-01'`, passing the threshold and being counted as unplayed.
    #[tokio::test]
    async fn null_air_date_episode_excluded_from_unplayed_count() {
        use chrono::Utc;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let cfg = api::ServerConfiguration {
            filter_by_digital_release_date: true,
            digital_release_buffer_days: 0,
            ..Default::default()
        };
        db::Settings::set_config(db, &cfg)
            .await
            .unwrap();

        let now = Utc::now().naive_utc();
        let series_imdb =
            db::NonEmptyString::try_new("tt_null_uc_001".to_string()).unwrap();

        let mut series = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: db::ExternalIds {
                    imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: None,
                episode: None,
            }),
            title: "NullDateCountSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            digital_released_at: Some(now - chrono::Duration::days(365)),
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let mut season = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Season,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: None,
            }),
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // ep1: released (past date)
        let mut ep1 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(1),
            }),
            title: "S1E1".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(1),
            digital_released_at: Some(now - chrono::Duration::days(7)),
            ..Default::default()
        };
        ep1.save(db)
            .await
            .unwrap();

        // ep2: no air date at all — anime, TVDB has no release date
        let mut ep2 = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(1),
                episode: Some(2),
            }),
            title: "S1E2".to_string(),
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series.id),
            parent_id: Some(season.id),
            parent_idx: Some(1),
            idx: Some(2),
            digital_released_at: None,
            released_at: None,
            ..Default::default()
        };
        ep2.save(db)
            .await
            .unwrap();

        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();

        // Mark ep1 (the only released episode) as played via the API.
        server
            .post(&format!("/users/{}/playeditems/{}", user.id, ep1.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        // Fetch the series with the filter active — unplayed_item_count must be 0.
        let threshold = cfg
            .release_date_threshold()
            .unwrap();
        let results = db::Media::get_by_filter(
            db,
            &db::MediaFilter {
                id: Some(vec![series.id]),
                include_user_state: true,
                user_id: Some(user.id),
                digital_released_before: Some(threshold),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let series_record = results
            .records
            .into_iter()
            .find(|m| m.id == series.id)
            .expect("series must be in result");

        assert_eq!(
            series_record.unplayed_item_count,
            Some(0),
            "null-date episode must not be counted as unplayed when the release-date filter is active"
        );
    }
}
