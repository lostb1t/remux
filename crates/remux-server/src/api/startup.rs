use axum::{Json, extract::State, response::IntoResponse};
use http::StatusCode;
use remux_macros::{get, post};

use crate::{AppState, IntoApiError, ResultExt, api};
use axum_anyhow::ApiResult as Result;

async fn require_wizard_incomplete(state: &AppState) -> Result<()> {
    let config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?;
    if config
        .is_startup_wizard_completed
        .unwrap_or(false)
    {
        return Err(anyhow::anyhow!("forbidden")
            .context_forbidden("Setup wizard is already completed."));
    }
    Ok(())
}

#[get("/startup/configuration")]
pub async fn get_startup_configuration(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    let config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?;
    Ok(Json(api::StartupConfiguration {
        server_name: config.server_name,
        preferred_metadata_language: config.preferred_metadata_language,
        metadata_country_code: config.metadata_country_code,
        default_web_client: Some(crate::web_client::normalize_web_client(
            config.default_web_client,
        )),
    }))
}

#[post("/startup/configuration")]
pub async fn post_startup_configuration(
    State(state): State<AppState>,
    Json(body): Json<api::StartupConfiguration>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    let api::StartupConfiguration {
        server_name,
        preferred_metadata_language,
        metadata_country_code,
        default_web_client,
    } = body;

    let mut config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?;
    config.server_name = server_name.or(config.server_name);
    config.preferred_metadata_language =
        preferred_metadata_language.or(config.preferred_metadata_language);
    config.metadata_country_code =
        metadata_country_code.or(config.metadata_country_code);
    config.default_web_client = Some(crate::web_client::normalize_web_client(
        default_web_client.or(config.default_web_client),
    ));
    crate::db::Settings::set_config(
        &state
            .ctx
            .db,
        &config,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /Startup/User
#[get("/startup/user")]
pub async fn get_startup_user(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    Ok(Json(api::StartupUser::default()))
}

/// POST /Startup/User — create the initial admin user
#[post("/startup/user")]
pub async fn post_startup_user(
    State(state): State<AppState>,
    Json(body): Json<api::StartupUser>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    let name = body
        .name
        .ok_or_else(|| {
            anyhow::anyhow!("name is required")
                .context_bad_request(
                    "Missing required field 'Name'. StartupUser expects PascalCase keys: {Name, Password, PasswordConfirm}.",
                )
        })?
        .into_inner();
    let password = body
        .password
        .ok_or_else(|| {
            anyhow::anyhow!("password is required")
                .context_bad_request(
                    "Missing required field 'Password'. StartupUser expects PascalCase keys: {Name, Password, PasswordConfirm}.",
                )
        })?;
    if password.is_empty() {
        return Err(anyhow::anyhow!("password is empty")
            .context_bad_request("Password must not be empty."));
    }
    // Optional confirmation check — only enforce when the client supplied it.
    // Some scripted clients omit PasswordConfirm; rejecting them silently broke
    // the wizard before this fix. Compare only when present.
    if let Some(confirm) = body.password_confirm.as_deref() {
        if confirm != password {
            return Err(anyhow::anyhow!("passwords do not match")
                .context_bad_request(
                    "PasswordConfirm does not match Password.",
                ));
        }
    }
    let mut user = crate::db::User::new_with_password(
        String::new(),
        name,
        &password,
        None,
    )?;
    user.is_admin = true;
    user.save_by_username(
        &state
            .ctx
            .db,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Startup/RemoteAccess — no-op, we don't configure remote access during wizard
#[post("/startup/remoteaccess")]
pub async fn post_startup_remote_access(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Startup/Complete — mark the wizard as done
#[post("/startup/complete")]
pub async fn post_startup_complete(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    require_wizard_incomplete(&state).await?;
    let mut config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?;
    config.is_startup_wizard_completed = Some(true);
    crate::db::Settings::set_config(
        &state
            .ctx
            .db,
        &config,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::integration_test::TestGuard;
    use crate::{Config, init_app_with_ctx};
    use serde_json::json;

    /// Like `new_test_server_with_config` but does **not** POST `/startup/complete`,
    /// so the wizard stays open and validation errors on `/startup/user` are reachable.
    async fn new_wizard_open_server() -> (axum_test::TestServer, TestGuard) {
        let (app, ctx) = init_app_with_ctx(Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            disable_dht: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let server = axum_test::TestServer::builder()
            .save_cookies()
            .mock_transport()
            .build(app)
            .unwrap();
        (server, TestGuard(ctx))
    }

    /// Lowercase JSON keys silently deserialized to `None` before the fix,
    /// so `POST /startup/user` returned 204 without creating any user.
    /// The fix requires PascalCase keys and now returns 400 when fields
    /// are missing.
    #[tokio::test]
    async fn startup_user_rejects_lowercase_json() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({ "name": "admin", "password": "secret123" }))
            .await;
        // Before the fix: HTTP 204 + no user created. After: HTTP 400.
        resp.assert_status_bad_request();
    }

    #[tokio::test]
    async fn startup_user_rejects_missing_name() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({ "Password": "secret123" }))
            .await;
        resp.assert_status_bad_request();
    }

    #[tokio::test]
    async fn startup_user_rejects_missing_password() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({ "Name": "admin" }))
            .await;
        resp.assert_status_bad_request();
    }

    #[tokio::test]
    async fn startup_user_rejects_empty_password() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({
                "Name": "admin",
                "Password": "",
            }))
            .await;
        resp.assert_status_bad_request();
    }

    #[tokio::test]
    async fn startup_user_rejects_mismatched_password_confirm() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({
                "Name": "admin",
                "Password": "secret123",
                "PasswordConfirm": "different",
            }))
            .await;
        resp.assert_status_bad_request();
    }

    #[tokio::test]
    async fn startup_user_accepts_valid_pascal_case_payload() {
        let (server, _ctx) = new_wizard_open_server().await;
        let resp = server
            .post("/startup/user")
            .json(&json!({
                "Name": "admin",
                "Password": "secret123",
            }))
            .await;
        resp.assert_status_no_content();
    }
}
