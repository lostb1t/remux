use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{get, post};

use crate::AppState;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, ResultExt};

#[get("/startup/configuration")]
pub async fn get_startup_configuration(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    Ok(Json(jellyfin::StartupConfiguration {
        server_name: config.server_name,
        preferred_metadata_language: config.preferred_metadata_language,
        metadata_country_code: config.metadata_country_code,
        default_web_client: Some(crate::web_client::normalize_web_client(
            config.default_web_client,
        )),
        aio_url: config.aio_url,
    }))
}

#[post("/startup/configuration")]
pub async fn post_startup_configuration(
    State(state): State<AppState>,
    Json(body): Json<jellyfin::StartupConfiguration>,
) -> Result<impl IntoResponse> {
    let jellyfin::StartupConfiguration {
        server_name,
        preferred_metadata_language,
        metadata_country_code,
        default_web_client,
        aio_url,
    } = body;

    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.server_name = server_name.or(config.server_name);
    config.preferred_metadata_language =
        preferred_metadata_language.or(config.preferred_metadata_language);
    config.metadata_country_code =
        metadata_country_code.or(config.metadata_country_code);
    if let Some(url) = aio_url {
        crate::aio::AioService::from_url(&url)
            .context_bad_request("Invalid AIO URL", "Could not build AIO client from the provided URL.")?
            .get_manifest()
            .await
            .context_bad_request("Invalid AIO URL", "Could not fetch manifest from the provided AIO URL. Check the URL is correct and the service is reachable.")?;
        config.aio_url = Some(url);
    }
    config.default_web_client = Some(crate::web_client::normalize_web_client(
        default_web_client.or(config.default_web_client),
    ));
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
        let mut user =
            crate::db::User::new_with_password(String::new(), name, &password, None)?;
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
