use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{get, post};

use crate::AppState;
use crate::jellyfin;
use axum_anyhow::ApiResult as Result;

#[get("/startup/configuration")]
pub async fn get_startup_configuration(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    Ok(Json(jellyfin::StartupConfiguration {
        server_name: config.server_name,
        preferred_metadata_language: config.preferred_metadata_language,
        metadata_country_code: config.metadata_country_code,
    }))
}


#[post("/startup/configuration")]
pub async fn post_startup_configuration(
    State(state): State<AppState>,
    Json(body): Json<jellyfin::StartupConfiguration>,
) -> Result<impl IntoResponse> {
    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.server_name = body.server_name.or(config.server_name);
    config.preferred_metadata_language = body.preferred_metadata_language.or(config.preferred_metadata_language);
    config.metadata_country_code = body.metadata_country_code.or(config.metadata_country_code);
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /Startup/User
#[get("/startup/user")]
pub async fn get_startup_user() -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::StartupUser::default()))
}

/// POST /Startup/User — create the initial admin user
#[post("/startup/user")]
pub async fn post_startup_user(
    State(state): State<AppState>,
    Json(body): Json<jellyfin::StartupUser>,
) -> Result<impl IntoResponse> {
    if let (Some(name), Some(password)) = (body.name, body.password) {
        let mut user = crate::db::User::new_with_password(String::new(), name, &password, None)?;
        user.is_admin = true;
        user.save_by_username(&state.ctx.db).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Startup/RemoteAccess — no-op, we don't configure remote access during wizard
#[post("/startup/remoteaccess")]
pub async fn post_startup_remote_access() -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Startup/Complete — mark the wizard as done
#[post("/startup/complete")]
pub async fn post_startup_complete(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.is_startup_wizard_completed = Some(true);
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;
    Ok(StatusCode::NO_CONTENT)
}
