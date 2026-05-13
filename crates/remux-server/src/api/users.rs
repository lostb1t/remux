use anyhow::Context;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::api::system::QuickConnectEntry;
use crate::common::{get_uuid, server_id};
use crate::db;
use crate::db::auth;
use crate::db::user::User;
use crate::ws::WsEvent;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};
use remux_sdks::remux::Username;

use super::items::{item, items, items_flat};
use super::mock_items;
use super::shows::userviews;

#[post("/users/{user_id}/configuration")]
pub async fn user_configuration_update(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(payload): Json<api::UserConfiguration>,
) -> Result<impl IntoResponse> {
    db::User::save_configuration(&state.ctx.db, &session.user.id, &payload).await?;
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
        db::User::get_by_id(&state.ctx.db, &user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found"))?
    } else {
        session.user
    };

    let result = db::JellyfinDisplayPrefs::get_by_filter(
        &state.ctx.db,
        &db::JellyfinDisplayPrefsFilter {
            id: Some(vec![id]),
            client: Some(q.client.clone()),
            user_id: Some(user.id),
            ..Default::default()
        },
    )
    .await?;

    let prefs = if let Some(record) = result.records.first() {
        record.clone()
    } else {
        db::JellyfinDisplayPrefs {
            client: Some(q.client),
            ..Default::default()
        }
    };

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
        db::User::get_by_id(&state.ctx.db, &user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found"))?
    } else {
        session.user
    };

    let prefs = db::JellyfinDisplayPrefs {
        id: id.clone(),
        user_id: user.id,
        client: Some(q.client.clone()),
        data: sqlx::types::Json(db::JellyfinDisplayPrefsData::from(payload)),
    };

    prefs.save(&state.ctx.db).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/authenticatebyname")]
pub async fn users_authenticatebyname(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
    Json(data): Json<api::AuthenticateUserByName>,
) -> Result<impl IntoResponse> {
    let user = User::authenticate(
        &state.ctx.db,
        data.username.as_deref().unwrap_or(""),
        data.pw.as_deref().unwrap_or(""),
    )
    .await?
    .context_unauthorized("not found", "not foubd")?;
    let device = auth::Device::new_from_header(auth_header, &user)?;
    device.save(&state.ctx.db).await?;

    let session_info = api::SessionInfoDto {
        id: Some(device.id.clone()),
        device_id: Some(device.id.clone()),
        device_name: Some(device.name.clone()),
        client: Some(device.app_name.clone()),
        application_version: Some(device.app_version.clone()),
        user_id: device.user_id.to_string(),
        user_name: Some(user.username.clone()),
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
    let mut user_dto = api::db_user_to_dto(user);
    user_dto.last_login_date = Some(now);
    user_dto.last_activity_date = Some(now);

    Ok(Json(api::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        session_info: Some(session_info),
        user: Some(user_dto),
    }))
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
        .context_unauthorized(
            "Unauthorized",
            "QuickConnect request not found or expired",
        )?;

    if !entry.authenticated {
        return Err(anyhow::anyhow!("not authenticated")).context_unauthorized(
            "Unauthorized",
            "QuickConnect request has not been approved yet",
        );
    }

    let user_id = entry
        .user_id
        .context_unauthorized("Unauthorized", "QuickConnect entry missing user")?;

    let user = db::User::get_by_id(&state.ctx.db, &user_id)
        .await?
        .context_unauthorized("Unauthorized", "User not found")?;

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
        app_version: auth_header.version.unwrap_or_else(|| "1.0".to_string()),
        user_id: user.id,
        access_token: get_uuid().to_string(),
        last_activity_at: None,
        capabilities: None,
        remote_ip: None,
    };
    device.save(&state.ctx.db).await?;

    // clean up store entries
    state.ctx.store.delete(format!("qc:{}", body.secret));
    state.ctx.store.delete(format!("qc:code:{}", entry.code));

    let session_info = api::SessionInfoDto {
        id: Some(device.id.clone()),
        device_id: Some(device.id.clone()),
        device_name: Some(device.name.clone()),
        client: Some(device.app_name.clone()),
        application_version: Some(device.app_version.clone()),
        user_id: device.user_id.to_string(),
        user_name: Some(user.username.clone()),
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
    let mut user_dto = api::db_user_to_dto(user);
    user_dto.last_login_date = Some(now);
    user_dto.last_activity_date = Some(now);

    Ok(Json(api::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        session_info: Some(session_info),
        user: Some(user_dto),
    }))
}

#[get("/users")]
pub async fn users(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let items = db::User::get_by_filter(
        &state.ctx.db,
        &db::UserFilter {
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|x| {
        let mut item = api::db_user_to_dto(x);
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
    Ok(Json(api::db_user_to_dto(session.user)).into_response())
}

#[post("/users/{user_id}/favoriteitems/{id}")]
pub async fn mark_favorite(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context("not foubd")?;
    let state = media.mark_favorite(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(state, &media)).into_response())
}

#[delete("/users/{user_id}/favoriteitems/{id}")]
pub async fn unmark_favorite(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context("not foubd")?;
    let state = media.unmark_favorite(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(state, &media)).into_response())
}

#[post("/userfavoriteitems/{id}")]
pub async fn mark_favorite_modern(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Item not found")?;
    let s = media.mark_favorite(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(s, &media)).into_response())
}

#[delete("/userfavoriteitems/{id}")]
pub async fn unmark_favorite_modern(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Item not found")?;
    let s = media.unmark_favorite(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(s, &media)).into_response())
}

#[post("/users/{user_id}/playeditems/{id}")]
pub async fn mark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context("not foubd")?;
    let state = media.mark_played(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(state, &media)).into_response())
}

#[delete("/users/{user_id}/playeditems/{id}")]
pub async fn unmark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context("not foubd")?;
    let state = media.mark_unplayed(&state.ctx.db, &session.user).await?;
    Ok(Json(api::db_state_to_dto(state, &media)).into_response())
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
    let password = payload.password.as_deref().unwrap_or("");
    let mut user = User::new_with_password(
        String::new(),
        payload.name.into_inner(),
        password,
        None,
    )?;
    user.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserUpdated(user.id));
    Ok((StatusCode::OK, Json(api::db_user_to_dto(user))).into_response())
}

#[delete("/users/{user_id}")]
pub async fn delete_user(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if user_id == session.user.id {
        return Err(anyhow::anyhow!("Cannot delete yourself")
            .context_bad_request("invalid", "cannot delete own account"));
    }
    db::User::delete(&state.ctx.db, &user_id).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserDeleted(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/password")]
pub async fn change_password(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<api::UpdateUserPassword>,
) -> Result<impl IntoResponse> {
    let is_self = user_id == session.user.id;
    let is_admin = session.user.is_admin;

    if !is_self && !is_admin {
        return Err(
            anyhow::anyhow!("Forbidden").context_unauthorized("forbidden", "forbidden")
        );
    }

    let mut user = db::User::get_by_id(&state.ctx.db, &user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User not found"))?;

    if is_self && !is_admin {
        let current = payload.current_pw.as_deref().unwrap_or("");
        if !user.verify_password(current)? {
            return Err(anyhow::anyhow!("Current password is incorrect")
                .context_unauthorized("invalid", "invalid password"));
        }
    }

    let new_pw = payload.new_pw.as_deref().unwrap_or("");
    user.set_password(new_pw)?;
    user.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/policy")]
pub async fn update_user_policy(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(user_id): Path<Uuid>,
    Json(policy): Json<api::UserPolicy>,
) -> Result<impl IntoResponse> {
    let mut user = db::User::get_by_id(&state.ctx.db, &user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User not found"))?;
    user.is_admin = policy.is_administrator;
    user.policy = Some(sqlx::types::Json(policy));
    user.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}")]
pub async fn update_user(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<api::UserDto>,
) -> Result<impl IntoResponse> {
    let is_self = user_id == session.user.id;
    if !is_self && !session.user.is_admin {
        return Err(
            anyhow::anyhow!("Forbidden").context_unauthorized("forbidden", "forbidden")
        );
    }
    let mut user = db::User::get_by_id(&state.ctx.db, &user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User not found"))?;
    let username = Username::try_new(payload.name)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context_bad_request("InvalidUsername", "Invalid username")?;
    user.username = username.into_inner();
    if let Some(config) = payload.configuration {
        user.configuration = Some(sqlx::types::Json(config));
    }
    user.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserUpdated(user_id));
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ===== Route aliases (same handler, different path) =====

#[get("/users/public")]
pub async fn users_public() -> Result<impl IntoResponse> {
    Ok(Json::<Vec<api::UserDto>>(vec![]).into_response())
}

#[get("/users/{user_id}")]
pub async fn users_get_by_id(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if user_id == session.user.id {
        return Ok(Json(api::db_user_to_dto(session.user)).into_response());
    }
    if !session.user.is_admin {
        return Err(
            anyhow::anyhow!("Forbidden").context_unauthorized("forbidden", "forbidden")
        );
    }
    let user = db::User::get_by_id(&state.ctx.db, &user_id)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!("User not found")
                .context_not_found("not found", "user not found")
        })?;
    Ok(Json(api::db_user_to_dto(user)).into_response())
}

#[get("/users/{user_id}/items/{id}")]
pub async fn users_items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    return Ok(
        Json(item(state, session, id, q.fields.as_deref()).await?).into_response()
    );
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
    q.user_id = Some(session.user.id);
    q.filters = Some(vec![api::ItemFilter::IsResumable]);
    if q.limit.is_none() {
        q.limit = Some(50);
    }
    let server_config = crate::db::Settings::get_config(&state.ctx.db).await.ok();
    let result = db::Media::get_by_jellyfin_filter(
        &state.ctx.db,
        &q,
        true,
        Some(&session.user),
        server_config.as_ref(),
        None,
    )
    .await?;
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result
            .records
            .into_iter()
            .map(api::db_media_to_item)
            .collect(),
        total_record_count: result.total_count as i64,
        start_index: q.start_index.unwrap_or(0),
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
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/users/{user_id}/items/{id}/intros")]
pub async fn users_items_intros(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
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

fn avatar_path(user_id: &Uuid) -> std::path::PathBuf {
    crate::base_data_dir()
        .join("meta")
        .join("avatars")
        .join(user_id.to_string())
}

pub fn user_has_avatar(user_id: &Uuid) -> bool {
    avatar_path(user_id).exists()
}

fn detect_image_content_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => "image/jpeg",
    }
}

/// Jellyfin clients send the image body base64-encoded. Decode it, falling
/// back to raw bytes if the content does not look like valid base64.
fn decode_image_body(body: &[u8]) -> Vec<u8> {
    use base64::Engine;
    // Strip optional data-URI prefix (data:image/jpeg;base64,...)
    let src = if let Some(pos) = body.iter().position(|&b| b == b',') {
        &body[pos + 1..]
    } else {
        body
    };
    base64::engine::general_purpose::STANDARD
        .decode(src)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(src))
        .unwrap_or_else(|_| body.to_vec())
}

async fn upload_avatar_for(user_id: &Uuid, body: Bytes) -> anyhow::Result<()> {
    let decoded = decode_image_body(&body);
    let path = avatar_path(user_id);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .context("failed to create avatars directory")?;
    tokio::fs::write(&path, &decoded)
        .await
        .context("failed to write avatar file")?;
    Ok(())
}

async fn delete_avatar_for(user_id: &Uuid) -> anyhow::Result<()> {
    let path = avatar_path(user_id);
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .context("failed to delete avatar file")?;
    }
    Ok(())
}

async fn serve_avatar_for(user_id: Uuid) -> Result<impl IntoResponse> {
    let path = avatar_path(&user_id);
    let bytes = tokio::fs::read(&path).await.map_err(|_| {
        anyhow::anyhow!("avatar not found")
            .context_not_found("not found", "avatar not found")
    })?;
    let content_type = detect_image_content_type(&bytes);
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
    Query(q): Query<UserImageQuery>,
) -> Result<impl IntoResponse> {
    let uid = q
        .user_id
        .or_else(|| q.tag.as_deref().and_then(|t| Uuid::parse_str(t).ok()))
        .context_bad_request("missing", "userId required")?;
    serve_avatar_for(uid).await
}

#[get("/users/{user_id}/images/{image_type}")]
pub async fn get_user_image_by_id(
    Path((user_id, _image_type)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    serve_avatar_for(user_id).await
}

#[get("/users/{user_id}/images/{image_type}/{index}")]
pub async fn get_user_image_by_id_indexed(
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
) -> Result<impl IntoResponse> {
    serve_avatar_for(user_id).await
}

// --- POST (upload) ---

#[post("/userimage")]
pub async fn upload_user_image(
    State(state): State<AppState>,
    session: auth::AuthSession,
    body: Bytes,
) -> Result<impl IntoResponse> {
    upload_avatar_for(&session.user.id, body)
        .await
        .context_internal("upload failed", "failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/images/{image_type}")]
pub async fn upload_user_image_legacy(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type)): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    upload_avatar_for(&user_id, body)
        .await
        .context_internal("upload failed", "failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/users/{user_id}/images/{image_type}/{index}")]
pub async fn upload_user_image_indexed(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
    body: Bytes,
) -> Result<impl IntoResponse> {
    upload_avatar_for(&user_id, body)
        .await
        .context_internal("upload failed", "failed to save avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- DELETE ---

#[delete("/userimage")]
pub async fn delete_user_image(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    delete_avatar_for(&session.user.id)
        .await
        .context_internal("delete failed", "failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[delete("/users/{user_id}/images/{image_type}")]
pub async fn delete_user_image_legacy(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    delete_avatar_for(&user_id)
        .await
        .context_internal("delete failed", "failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[delete("/users/{user_id}/images/{image_type}/{index}")]
pub async fn delete_user_image_indexed(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, _image_type, _index)): Path<(Uuid, String, usize)>,
) -> Result<impl IntoResponse> {
    delete_avatar_for(&user_id)
        .await
        .context_internal("delete failed", "failed to delete avatar")?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server, new_test_server,
    };
    use http::header::HeaderValue;
    use serde_json::json;

    #[tokio::test]
    async fn test_authenticate_valid_credentials() {
        let (server, _ctx) = new_test_server().await.unwrap();

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
        assert!(body["AccessToken"].as_str().is_some_and(|t| !t.is_empty()));
        assert_eq!(body["User"]["Name"], "test");
    }

    #[tokio::test]
    async fn test_authenticate_wrong_password() {
        let (server, _ctx) = new_test_server().await.unwrap();

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
        let (server, _ctx) = new_test_server().await.unwrap();

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
        let user_id = user["Id"].as_str().unwrap();

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
}
