use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::Redirect;
use axum::response::IntoResponse;
use remux_macros::{delete, get, post};
use axum_extra::extract::Query;
use http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::db::user::User;
use crate::jellyfin;
use crate::utils::server_id;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

use super::mock_items;
use super::system::system_info_public;
use super::items::{items, items_flat, item};
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

    Ok(Json(jellyfin::DisplayPreferencesDto::from(prefs)))
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
    let user = User::authenticate(&state.ctx.db, &data.username, &data.pw)
        .await?
        .context_unauthorized("not found", "not foubd")?;
    let device = auth::Device::new_from_header(auth_header, &user)?;
    device.save(&state.ctx.db).await?;

    Ok(Json(jellyfin::AuthenticationResult {
        access_token: Some(device.access_token),
        server_id: server_id(),
        user: Some(user.into()),
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
        let mut item: jellyfin::UserDto = x.into();
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
    Ok(Json(jellyfin::UserDto::from(session.user)).into_response())
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
    Ok(Json(jellyfin::UserItemDataDto::from(state)).into_response())
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
    Ok(Json(jellyfin::UserItemDataDto::from(state)).into_response())
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
    Ok(Json(jellyfin::UserItemDataDto::from(state)).into_response())
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
    let state = media.mark_played(&state.ctx.db, &session.user).await?;
    Ok(Json(jellyfin::UserItemDataDto::from(state)).into_response())
}

#[get("/users/{user_id}/groupingoptions")]
pub async fn users_groupingoptions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json::<Vec<jellyfin::SpecialViewOptionDto>>(vec![]))
}

// ===== Route aliases (same handler, different path) =====

#[get("/users/public")]
pub async fn users_public(State(state): State<AppState>) -> Result<impl IntoResponse> {
    system_info_public(State(state)).await
}

#[get("/users/{user_id}")]
pub async fn users_get_by_id(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    users_me(State(state), session).await
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

// ===== Named stubs (empty responses for unimplemented endpoints) =====

#[get("/users/{user_id}/items/resume")]
pub async fn users_items_resume(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/users/{user_id}/items/similar")]
pub async fn users_items_similar(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/users/{user_id}/intros")]
pub async fn users_intros(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/users/{user_id}/items/{id}/intros")]
pub async fn users_items_intros(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/useritems/resume")]
pub async fn useritems_resume(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use http::header::HeaderValue;
    use serde_json::json;

    const AUTH_HEADER: &str = "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\"";

    async fn authenticated_server() -> (axum_test::TestServer, String) {
        let server = crate::integration_test::new_test_server().await.unwrap();

        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({
                "Username": "test",
                "Pw": "test"
            }))
            .await;

        let body: serde_json::Value = resp.json();
        let token = body["AccessToken"].as_str().unwrap().to_string();
        (server, token)
    }

    fn auth_header_with_token(token: &str) -> String {
        format!(
            "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\", Token=\"{}\"",
            token
        )
    }

    #[tokio::test]
    async fn test_update_display_preferences() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // POST to save display preferences
        let resp = server
            .post("/displaypreferences/usersettings")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Get user ID from /users/me
        let resp = server
            .get("/users/me")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .await;

        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        let user_id = user["Id"].as_str().unwrap();

        // POST user configuration
        let resp = server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .await;

        resp.assert_status_ok();
        let user: serde_json::Value = resp.json();
        assert_eq!(user["Configuration"]["SubtitleLanguagePreference"], "eng");
        assert_eq!(user["Configuration"]["EnableNextEpisodeAutoPlay"], true);
        assert_eq!(user["Configuration"]["HidePlayedInLatest"], true);
    }
}
