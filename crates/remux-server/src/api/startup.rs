use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{get, post};

use crate::AppState;
use crate::api;
use crate::{IntoApiError, ResultExt};
use axum_anyhow::ApiResult as Result;

async fn require_wizard_incomplete(state: &AppState) -> Result<()> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    if config.is_startup_wizard_completed.unwrap_or(false) {
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
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
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

    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.server_name = server_name.or(config.server_name);
    config.preferred_metadata_language =
        preferred_metadata_language.or(config.preferred_metadata_language);
    config.metadata_country_code =
        metadata_country_code.or(config.metadata_country_code);
    config.default_web_client = Some(crate::web_client::normalize_web_client(
        default_web_client.or(config.default_web_client),
    ));
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;

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
    if let (Some(name), Some(password)) = (body.name, body.password) {
        let mut user = crate::db::User::new_with_password(
            String::new(),
            name.into_inner(),
            &password,
            None,
        )?;
        user.is_admin = true;
        user.save_by_username(&state.ctx.db).await?;
    }
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
    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.is_startup_wizard_completed = Some(true);
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;
    Ok(StatusCode::NO_CONTENT)
}
