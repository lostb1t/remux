use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::db::user::User;
use crate::jellyfin;
use crate::utils::server_id;
use crate::ws::WsEvent;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};

use super::items::{item, items, items_flat};
use super::mock_items;
use super::shows::userviews;

#[post("/users/{user_id}/configuration")]
pub async fn user_configuration_update(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(payload): Json<jellyfin::UserConfiguration>,
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

    Ok(Json(jellyfin::db_display_prefs_to_dto(prefs)))
}

#[post("/displaypreferences/{id}")]
pub async fn update_display_preferences(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<String>,
    Query(q): Query<DisplayPrefQuery>,
    Json(payload): Json<jellyfin::DisplayPreferencesDto>,
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
    Json(data): Json<jellyfin::AuthenticateUserByName>,
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

    Ok(Json(jellyfin::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        user: Some(jellyfin::db_user_to_dto(user)),
        ..Default::default()
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
        let mut item = jellyfin::db_user_to_dto(x);
        //item.type_ = jellyfin::MediaType::CollectionFolder;
        //item.collection_type = Some(jellyfin::CollectionType::Movies);
        item
    })
    .collect::<Vec<jellyfin::UserDto>>();

    Ok(Json(items))
}

#[get("/userimage")]
pub async fn user_image(
    State(state): State<AppState>,
    // Query(q): Query<jellyfin::ImageQuery>,
) -> Result<impl IntoResponse> {
    let url = Some("https://placehold.co/600x400".to_string());

    Ok(Redirect::temporary(url.unwrap().as_str()))
}

#[get("/users/me")]
pub async fn users_me(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::db_user_to_dto(session.user)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(state)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(state)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(s)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(s)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(state)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(state)).into_response())
}

#[get("/users/{user_id}/groupingoptions")]
pub async fn users_groupingoptions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json::<Vec<jellyfin::SpecialViewOptionDto>>(vec![]))
}

#[post("/users/new")]
pub async fn create_user(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<jellyfin::CreateUserByName>,
) -> Result<impl IntoResponse> {
    let password = payload.password.as_deref().unwrap_or("");
    let mut user =
        User::new_with_password(String::new(), payload.name, password, None)?;
    user.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(WsEvent::UserUpdated(user.id));
    Ok((StatusCode::OK, Json(jellyfin::db_user_to_dto(user))).into_response())
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
    Json(payload): Json<jellyfin::UpdateUserPassword>,
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
    Json(policy): Json<jellyfin::UserPolicy>,
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
    Json(payload): Json<jellyfin::UserDto>,
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
    user.username = payload.name;
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
    Ok(Json::<Vec<jellyfin::UserDto>>(vec![]).into_response())
}

#[get("/users/{user_id}")]
pub async fn users_get_by_id(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(user_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    if user_id == session.user.id {
        return Ok(Json(jellyfin::db_user_to_dto(session.user)).into_response());
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
    Ok(Json(jellyfin::db_user_to_dto(user)).into_response())
}

#[get("/users/{user_id}/items/{id}")]
pub async fn users_items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path((user_id, id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, session, id).await?).into_response());
}

#[get("/users/{user_id}/items")]
pub async fn users_items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    items(State(state), session, Query(q)).await
}

#[get("/users/{user_id}/items/latest")]
pub async fn users_items_latest(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
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

#[get("/users/{user_id}/items/resume")]
pub async fn users_items_resume(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
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
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
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
                "EnableNextEpisodeAutoPlay": true
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
