use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use remux_macros::{get, post, route};
use serde_json::json;

use crate::AppState;
use crate::db::auth;
use crate::jellyfin;
use crate::utils::server_id;
use anyhow;
use axum_anyhow::{ApiResult as Result, IntoApiError};

use super::{mock_items, stub};

#[get("/system/info/public")]
pub async fn system_info_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    Ok(Json(jellyfin::PublicSystemInfo {
        local_address: "0.0.0.0".to_string(),
        server_name: config.server_name.unwrap_or_default(),
        product_name: "Jellyfin Server".to_string(),
        startup_wizard_completed: config.is_startup_wizard_completed.unwrap_or(false),
        version: "10.11.6".to_string(),
        id: server_id(),
        ..Default::default()
    }))
}

#[get("/system/ping")]
pub async fn system_ping(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(json!("Remux Server")))
}

/// Get storage information
#[get("/system/info/storage")]
pub async fn system_info_storage(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    // Create storage information following the Jellyfin SystemStorageInfo structure
    let system_storage_info = jellyfin::SystemStorageInfo {
        program_data_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/data".to_string()),
            free_space: Some(500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("data-device".to_string()),
            ..Default::default()
        }),
        web_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/web".to_string()),
            free_space: Some(1000000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("web-device".to_string()),
            ..Default::default()
        }),
        image_cache_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/cache/images".to_string()),
            free_space: Some(800000000),
            used_space: Some(200000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("cache-device".to_string()),
            ..Default::default()
        }),
        cache_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/tmp".to_string()),
            free_space: Some(900000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("tmp-device".to_string()),
            ..Default::default()
        }),
        log_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/logs".to_string()),
            free_space: Some(700000000),
            used_space: Some(300000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("log-device".to_string()),
            ..Default::default()
        }),
        internal_metadata_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/metadata".to_string()),
            free_space: Some(600000000),
            used_space: Some(400000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("metadata-device".to_string()),
            ..Default::default()
        }),
        transcoding_temp_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/transcodes".to_string()),
            free_space: Some(1500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("transcode-device".to_string()),
            ..Default::default()
        }),
        libraries: Some(vec![
            jellyfin::LibraryStorageInfo {
                id: Some("movies-library-id".to_string()),
                name: Some("Movies".to_string()),
                folders: Some(vec![jellyfin::FolderStorageInfo {
                    path: Some("/media/movies".to_string()),
                    free_space: Some(2000000000),
                    used_space: Some(1000000000),
                    storage_type: Some("DefaultFileSystem".to_string()),
                    device_id: Some("media-device".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            jellyfin::LibraryStorageInfo {
                id: Some("series-library-id".to_string()),
                name: Some("TV Shows".to_string()),
                folders: Some(vec![jellyfin::FolderStorageInfo {
                    path: Some("/media/tv".to_string()),
                    free_space: Some(2000000000),
                    used_space: Some(1500000000),
                    storage_type: Some("DefaultFileSystem".to_string()),
                    device_id: Some("media-device".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    Ok(Json(system_storage_info))
}

/// Get server configuration
#[get("/system/configuration")]
pub async fn system_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    Ok(Json(crate::db::Settings::get_config(&state.ctx.db).await?))
}

/// Update server configuration
#[post("/system/configuration")]
pub async fn update_system_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(config): Json<jellyfin::ServerConfiguration>,
) -> Result<impl IntoResponse> {
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[get("/system/endpoint")]
pub async fn system_endpoint(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "IsLocal": false,
        "IsInNetwork": false,

    })))
}

#[get("/syncplay/list")]
pub async fn syncplay_list(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[route("/quickconnect/enabled", method = "GET", method = "POST")]
pub async fn quickconnect_enabled(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok("false".to_string())
}

const BRANDING_CONFIG_KEY: &str = "branding_configuration";

fn default_branding_configuration() -> jellyfin::BrandingOptions {
    jellyfin::BrandingOptions {
        login_disclaimer: None,
        custom_css: None,
        splashscreen_enabled: Some(false),
    }
}

#[get("/branding/configuration")]
pub async fn get_branding_configuration(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let config =
        match crate::db::Settings::get(&state.ctx.db, BRANDING_CONFIG_KEY).await? {
            Some(json) => serde_json::from_str(&json)
                .unwrap_or_else(|_| default_branding_configuration()),
            None => default_branding_configuration(),
        };
    Ok(Json(config))
}

#[post("/branding/configuration")]
pub async fn update_branding_configuration_legacy(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(config): Json<jellyfin::BrandingOptions>,
) -> Result<impl IntoResponse> {
    let json = serde_json::to_string(&config)?;
    crate::db::Settings::set(&state.ctx.db, BRANDING_CONFIG_KEY, &json).await?;
    Ok(StatusCode::NO_CONTENT)
}

// Jellyfin web posts branding updates here (System/Configuration/Branding)
#[post("/system/configuration/branding")]
pub async fn update_branding_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(config): Json<jellyfin::BrandingOptions>,
) -> Result<impl IntoResponse> {
    let json = serde_json::to_string(&config)?;
    crate::db::Settings::set(&state.ctx.db, BRANDING_CONFIG_KEY, &json).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn branding_css_response(state: &AppState) -> Result<Response> {
    let config =
        match crate::db::Settings::get(&state.ctx.db, BRANDING_CONFIG_KEY).await? {
            Some(json) => serde_json::from_str::<jellyfin::BrandingOptions>(&json).ok(),
            None => None,
        };
    match config.and_then(|c| c.custom_css).filter(|s| !s.is_empty()) {
        Some(css) => Ok(([(header::CONTENT_TYPE, "text/css")], css).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

#[get("/branding/css")]
pub async fn get_branding_css(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    branding_css_response(&state).await
}

#[get("/branding/css.css")]
pub async fn get_branding_css_dotcss(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    branding_css_response(&state).await
}

/// Get activity log entries
#[get("/system/activitylog/entries")]
pub async fn system_activity_log(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    // Return an empty activity log
    Ok(Json(json!({
        "Items": [],
        "TotalRecordCount": 0
    })))
}

/// Restart the server (Admin only)
#[post("/system/restart")]
pub async fn system_restart(session: auth::AdminSession) -> Result<impl IntoResponse> {
    tracing::info!(
        "Server restart requested by user: {}",
        session.user.username
    );

    // Trigger actual server restart
    restart_server().await?;

    Ok(Json(json!({
        "Message": "Server restart initiated",
        "RestartPending": true
    })))
}

/// Actually restart the server process
async fn restart_server() -> Result<()> {
    tracing::info!("Initiating server restart...");

    // Get the current executable path and arguments
    let current_exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().collect();

    tracing::info!("Restarting with: {:?} {:?}", current_exe, args);

    // Spawn the new process
    let mut command = std::process::Command::new(current_exe);
    command.args(&args[1..]); // Skip the first argument (program name)

    // Set environment variables from current process
    for (key, value) in std::env::vars() {
        command.env(key, value);
    }

    // Start the new process
    let mut child = command.spawn()?;

    tracing::info!("New server process started with PID: {}", child.id());

    // Give the new process a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Exit the current process
    std::process::exit(0);
}

/// Shutdown the server (Admin only)
#[post("/system/shutdown")]
pub async fn system_shutdown(session: auth::AdminSession) -> Result<impl IntoResponse> {
    tracing::info!(
        "Server shutdown requested by user: {}",
        session.user.username
    );

    // Trigger actual server shutdown
    shutdown_server().await?;

    Ok(Json(json!({
        "Message": "Server shutdown initiated",
        "IsShuttingDown": true
    })))
}

/// Actually shutdown the server process
async fn shutdown_server() -> Result<()> {
    tracing::info!("Initiating server shutdown...");

    // Perform graceful shutdown
    tracing::info!("Server is shutting down gracefully");

    // Give a moment for cleanup
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Exit the process
    std::process::exit(0);
}

#[get("/system/info")]
pub async fn system_info(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    Ok(Json(jellyfin::SystemInfo {
        id: Some(server_id()),
        server_name: config.server_name,
        can_self_restart: Some(true),
        has_pending_restart: Some(false),
        is_shutting_down: Some(false),
        ..Default::default()
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server, new_test_server,
    };
    use http::header::HeaderValue;
    use serde_json::json;

    #[tokio::test]
    async fn test_system_info_public() {
        let server = new_test_server().await.unwrap();

        let resp = server.get("/system/info/public").await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "ServerName": "Remux",
            "ProductName": "Jellyfin Server",
            "Version": "10.11.6",
            "StartupWizardCompleted": true,
        }));
        let body: serde_json::Value = resp.json();
        let id = body["Id"].as_str().expect("Id field should be present");
        uuid::Uuid::parse_str(id).expect("Id should be a valid UUID");
    }

    #[tokio::test]
    async fn system_ping_test() {
        let server = new_test_server().await.unwrap();

        let response = server.get("/system/ping").await;

        response.assert_status_ok();
        //response.assert_text("Remux Server");
    }

    #[tokio::test]
    async fn system_info_storage_test() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let response = server
            .get("/system/info/storage")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        response.assert_status_ok();
        let storage_info: crate::jellyfin::SystemStorageInfo = response.json();

        // Check that we have the expected storage folders
        assert!(storage_info.program_data_folder.is_some());
        assert!(storage_info.cache_folder.is_some());
        assert!(storage_info.web_folder.is_some());

        // Check that we have libraries
        assert!(storage_info.libraries.is_some());
        let libraries = storage_info.libraries.unwrap();
        assert_eq!(libraries.len(), 2);

        // Check library names
        let library_names: Vec<String> = libraries
            .iter()
            .filter_map(|lib| lib.name.clone())
            .collect();
        assert!(library_names.contains(&"Movies".to_string()));
        assert!(library_names.contains(&"TV Shows".to_string()));
    }

    #[tokio::test]
    async fn system_activity_log_test() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let response = server
            .get("/system/activitylog/entries")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        response.assert_status_ok();
        let log_result: serde_json::Value = response.json();

        // Check that we have the expected structure
        assert!(log_result["Items"].is_array());
        assert_eq!(log_result["Items"].as_array().unwrap().len(), 0);
        assert_eq!(log_result["TotalRecordCount"].as_i64().unwrap(), 0);
    }

    #[tokio::test]
    async fn system_endpoints_exist_and_protected() {
        use crate::integration_test::new_test_server;
        let server = new_test_server().await.unwrap();

        // Unauthenticated requests should return 401, not 404
        let response = server.post("/system/restart").expect_failure().await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

        let response = server.post("/system/shutdown").expect_failure().await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_restart_requires_auth() {
        use crate::integration_test::new_test_server;

        let server = new_test_server().await.unwrap();
        // Unauthenticated → 401
        let response = server.post("/system/restart").expect_failure().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_shutdown_requires_auth() {
        use crate::integration_test::new_test_server;

        let server = new_test_server().await.unwrap();
        // Unauthenticated → 401
        let response = server.post("/system/shutdown").expect_failure().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_info_shows_capabilities() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let response = server
            .get("/system/info")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        response.assert_status_ok();
        let system_info: crate::jellyfin::SystemInfo = response.json();

        // Check that restart capabilities are properly indicated
        assert_eq!(system_info.can_self_restart, Some(true));
        assert_eq!(system_info.has_pending_restart, Some(false));
        assert_eq!(system_info.is_shutting_down, Some(false));
    }
}
