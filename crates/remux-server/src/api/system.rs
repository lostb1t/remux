use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use remux_macros::{get, post, route};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::common::{self, get_uuid, server_id};
use crate::db::{self, auth};
use anyhow;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};

use super::mock_items;

#[get("/system/info/public")]
pub async fn system_info_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    let config = crate::db::Settings::get_config(&state.ctx.db).await?;
    Ok(Json(api::PublicSystemInfo {
        // todo
        local_address: String::new(),
        server_name: config.server_name.unwrap_or_default(),
        product_name: "Jellyfin Server".to_string(),
        startup_wizard_completed: config.is_startup_wizard_completed.unwrap_or(false),
        // some clients dont like adding a suffix like "-remux"
        version: "10.11.8".to_string(),
        remux_version: env!("CARGO_PKG_VERSION").to_string(),
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
    let system_storage_info = api::SystemStorageInfo {
        program_data_folder: Some(api::FolderStorageInfo {
            path: Some("/data".to_string()),
            free_space: Some(500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("data-device".to_string()),
            ..Default::default()
        }),
        web_folder: Some(api::FolderStorageInfo {
            path: Some("/web".to_string()),
            free_space: Some(1000000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("web-device".to_string()),
            ..Default::default()
        }),
        image_cache_folder: Some(api::FolderStorageInfo {
            path: Some("/cache/images".to_string()),
            free_space: Some(800000000),
            used_space: Some(200000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("cache-device".to_string()),
            ..Default::default()
        }),
        cache_folder: Some(api::FolderStorageInfo {
            path: Some("/tmp".to_string()),
            free_space: Some(900000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("tmp-device".to_string()),
            ..Default::default()
        }),
        log_folder: Some(api::FolderStorageInfo {
            path: Some("/logs".to_string()),
            free_space: Some(700000000),
            used_space: Some(300000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("log-device".to_string()),
            ..Default::default()
        }),
        internal_metadata_folder: Some(api::FolderStorageInfo {
            path: Some("/metadata".to_string()),
            free_space: Some(600000000),
            used_space: Some(400000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("metadata-device".to_string()),
            ..Default::default()
        }),
        transcoding_temp_folder: Some(api::FolderStorageInfo {
            path: Some("/transcodes".to_string()),
            free_space: Some(1500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("transcode-device".to_string()),
            ..Default::default()
        }),
        libraries: Some(vec![
            api::LibraryStorageInfo {
                id: Some("movies-library-id".to_string()),
                name: Some("Movies".to_string()),
                folders: Some(vec![api::FolderStorageInfo {
                    path: Some("/media/movies".to_string()),
                    free_space: Some(2000000000),
                    used_space: Some(1000000000),
                    storage_type: Some("DefaultFileSystem".to_string()),
                    device_id: Some("media-device".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            api::LibraryStorageInfo {
                id: Some("series-library-id".to_string()),
                name: Some("TV Shows".to_string()),
                folders: Some(vec![api::FolderStorageInfo {
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
    let mut config = crate::db::Settings::get_config(&state.ctx.db).await?;
    config.default_web_client = Some(crate::web_client::normalize_web_client(
        config.default_web_client,
    ));
    Ok(Json(config))
}

/// Update server configuration
#[post("/system/configuration")]
pub async fn update_system_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(mut config): Json<api::ServerConfiguration>,
) -> Result<impl IntoResponse> {
    config.default_web_client = Some(crate::web_client::normalize_web_client(
        config.default_web_client,
    ));
    // Apply P2P speed limits before saving so they take effect immediately.
    if config.p2p_enabled.unwrap_or(true) {
        state.ctx.torrent.update_limits(
            config.p2p_upload_speed_kbps.unwrap_or(0),
            config.p2p_download_speed_kbps.unwrap_or(0),
        );
    }
    crate::db::Settings::set_config(&state.ctx.db, &config).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Get encoding configuration
#[get("/system/configuration/encoding")]
pub async fn get_encoding_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let opts = crate::db::Settings::get_encoding_config(&state.ctx.db).await?;
    Ok(Json(opts))
}

/// Update encoding configuration
#[post("/system/configuration/encoding")]
pub async fn update_encoding_configuration(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(opts): Json<api::EncodingOptions>,
) -> Result<impl IntoResponse> {
    crate::db::Settings::set_encoding_config(&state.ctx.db, &opts).await?;
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
    let cfg = db::Settings::get_config(&state.ctx.db).await?;
    let enabled = cfg.quick_connect_available.unwrap_or(true);
    Ok(Json(enabled))
}

#[derive(Clone)]
pub struct QuickConnectEntry {
    pub code: String,
    pub authenticated: bool,
    pub user_id: Option<Uuid>,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
    pub date_added: chrono::DateTime<chrono::Utc>,
}

#[post("/quickconnect/initiate")]
pub async fn quickconnect_initiate(
    State(state): State<AppState>,
    auth_header: auth::JellyfinAuthHeader,
) -> Result<impl IntoResponse> {
    let cfg = db::Settings::get_config(&state.ctx.db).await?;
    if !cfg.quick_connect_available.unwrap_or(true) {
        return Err(anyhow::anyhow!("QuickConnect is disabled"))
            .context_forbidden("Forbidden", "QuickConnect is disabled on this server");
    }

    let secret = get_uuid().simple().to_string();
    let code = format!("{:06}", get_uuid().as_u128() % 1_000_000);

    let device_id = auth_header.device_id.unwrap_or_default();
    let device_name = auth_header.device.unwrap_or_default();
    let app_name = auth_header.client.unwrap_or_default();
    let app_version = auth_header.version.unwrap_or_default();

    let date_added = chrono::Utc::now();
    let entry = QuickConnectEntry {
        code: code.clone(),
        authenticated: false,
        user_id: None,
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        app_name: app_name.clone(),
        app_version: app_version.clone(),
        date_added,
    };
    state
        .ctx
        .store
        .save(format!("qc:{secret}"), entry, Duration::from_secs(600));
    state.ctx.store.save(
        format!("qc:code:{code}"),
        secret.clone(),
        Duration::from_secs(600),
    );

    Ok(Json(api::QuickConnectResult {
        secret,
        code,
        authenticated: false,
        date_added,
        authentication_token: None,
        device_id: Some(device_id),
        device_name: Some(device_name),
        app_name: Some(app_name),
        app_version: Some(app_version),
    }))
}

#[derive(Deserialize)]
pub struct QuickConnectSecretQuery {
    #[serde(rename = "Secret", alias = "secret")]
    pub secret: String,
}

#[derive(Deserialize)]
pub struct QuickConnectCodeQuery {
    #[serde(rename = "Code", alias = "code")]
    pub code: String,
}

#[get("/quickconnect/connect")]
pub async fn quickconnect_connect(
    State(state): State<AppState>,
    Query(q): Query<QuickConnectSecretQuery>,
) -> Result<impl IntoResponse> {
    let entry = state
        .ctx
        .store
        .get::<QuickConnectEntry>(format!("qc:{}", q.secret))
        .context_not_found("NotFound", "QuickConnect request not found or expired")?;

    Ok(Json(api::QuickConnectResult {
        secret: q.secret.clone(),
        code: entry.code.clone(),
        authenticated: entry.authenticated,
        authentication_token: if entry.authenticated {
            Some(q.secret.clone())
        } else {
            None
        },
        date_added: entry.date_added,
        device_id: Some(entry.device_id),
        device_name: Some(entry.device_name),
        app_name: Some(entry.app_name),
        app_version: Some(entry.app_version),
    }))
}

#[post("/quickconnect/authorize")]
pub async fn quickconnect_authorize(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<QuickConnectCodeQuery>,
) -> Result<impl IntoResponse> {
    let secret = state
        .ctx
        .store
        .get::<String>(format!("qc:code:{}", q.code))
        .context_not_found("NotFound", "QuickConnect code not found or expired")?;

    let entry = state
        .ctx
        .store
        .get::<QuickConnectEntry>(format!("qc:{secret}"))
        .context_not_found("NotFound", "QuickConnect request not found or expired")?;

    state.ctx.store.save(
        format!("qc:{secret}"),
        QuickConnectEntry {
            authenticated: true,
            user_id: Some(session.user.id),
            ..entry
        },
        Duration::from_secs(300),
    );

    Ok(Json(true))
}

const BRANDING_CONFIG_KEY: &str = "branding_configuration";

fn default_branding_configuration() -> api::BrandingOptions {
    api::BrandingOptions {
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
    Json(config): Json<api::BrandingOptions>,
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
    Json(config): Json<api::BrandingOptions>,
) -> Result<impl IntoResponse> {
    let json = serde_json::to_string(&config)?;
    crate::db::Settings::set(&state.ctx.db, BRANDING_CONFIG_KEY, &json).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn branding_css_response(state: &AppState) -> Result<Response> {
    let config =
        match crate::db::Settings::get(&state.ctx.db, BRANDING_CONFIG_KEY).await? {
            Some(json) => serde_json::from_str::<api::BrandingOptions>(&json).ok(),
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

/// Return the current UTC time (no auth required — Jellyfin calls this before login)
#[get("/getutctime")]
pub async fn get_utc_time() -> impl IntoResponse {
    use chrono::Utc;
    let now = Utc::now();
    Json(crate::api::UtcTimeResponse {
        request_reception_time: now,
        response_transmission_time: Utc::now(),
    })
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
    Ok(Json(api::SystemInfo {
        id: Some(server_id()),
        server_name: config.server_name,
        product_name: "Jellyfin Server".to_string(),
        version: "10.11.8".to_string(),
        remux_version: env!("CARGO_PKG_VERSION").to_string(),
        can_self_restart: Some(true),
        has_pending_restart: false,
        is_shutting_down: false,
        supports_library_monitor: true,
        web_socket_port_number: 3000,
        ..Default::default()
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server, new_test_server,
    };
    use axum_test::expect_json;
    use http::header::HeaderValue;
    use serde_json::json;

    #[tokio::test]
    async fn test_system_info_public() {
        let (server, _ctx) = new_test_server().await.unwrap();

        let resp = server.get("/system/info/public").await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "Id": expect_json::uuid(),
            "LocalAddress": "",
            "ServerName": "Remux",
            "ProductName": "Jellyfin Server",
            "Version": "10.11.8",
            "RemuxVersion": env!("CARGO_PKG_VERSION"),
            "StartupWizardCompleted": true,
        }));
    }

    #[tokio::test]
    async fn system_endpoints_exist_and_protected() {
        use crate::integration_test::new_test_server;
        let (server, _ctx) = new_test_server().await.unwrap();

        // Unauthenticated requests should return 401, not 404
        let response = server.post("/system/restart").expect_failure().await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

        let response = server.post("/system/shutdown").expect_failure().await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_restart_requires_auth() {
        use crate::integration_test::new_test_server;

        let (server, _ctx) = new_test_server().await.unwrap();
        // Unauthenticated → 401
        let response = server.post("/system/restart").expect_failure().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_shutdown_requires_auth() {
        use crate::integration_test::new_test_server;

        let (server, _ctx) = new_test_server().await.unwrap();
        // Unauthenticated → 401
        let response = server.post("/system/shutdown").expect_failure().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_info_shows_capabilities() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let response = server
            .get("/system/info")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        response.assert_status_ok();
        let system_info: crate::api::SystemInfo = response.json();

        // Check that restart capabilities are properly indicated
        assert_eq!(system_info.can_self_restart, Some(true));
        assert_eq!(system_info.has_pending_restart, false);
        assert_eq!(system_info.is_shutting_down, false);
    }

    // --- GET /system/configuration ---

    #[tokio::test]
    async fn system_configuration_requires_auth() {
        let (server, _ctx) = new_test_server().await.unwrap();
        server
            .get("/system/configuration")
            .expect_failure()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn system_configuration_get_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .get("/system/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "ServerName": "Remux",
            "IsPortAuthorized": true,
        }));
    }

    #[tokio::test]
    async fn system_configuration_update_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Read current config
        let resp = server
            .get("/system/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let mut config: serde_json::Value = resp.json();

        // Modify ServerName
        config["ServerName"] = serde_json::Value::String("TestUpdated".to_string());

        // POST modified config → 204
        let post_resp = server
            .post("/system/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&config)
            .await;
        post_resp.assert_status(StatusCode::NO_CONTENT);

        // GET again and verify the change persisted
        let resp2 = server
            .get("/system/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp2.assert_status_ok();
        resp2.assert_json_contains(&json!({ "ServerName": "TestUpdated" }));
    }

    // --- GET /system/endpoint ---

    #[tokio::test]
    async fn system_endpoint_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        let resp = server.get("/system/endpoint").await;

        resp.assert_status_ok();
        resp.assert_json(&json!({
            "IsLocal": false,
            "IsInNetwork": false,
        }));
    }

    // --- GET /syncplay/list ---

    #[tokio::test]
    async fn syncplay_list_requires_auth() {
        let (server, _ctx) = new_test_server().await.unwrap();
        server
            .get("/syncplay/list")
            .expect_failure()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn syncplay_list_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .get("/syncplay/list")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
    }

    // --- GET+POST /quickconnect/enabled ---

    #[tokio::test]
    async fn quickconnect_enabled_get_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        let resp = server.get("/quickconnect/enabled").await;

        resp.assert_status_ok();
        assert!(resp.text().contains("true"));
    }

    #[tokio::test]
    async fn quickconnect_enabled_post_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        let resp = server.post("/quickconnect/enabled").await;

        resp.assert_status_ok();
        assert!(resp.text().contains("true"));
    }

    // --- GET /branding/configuration ---

    #[tokio::test]
    async fn branding_configuration_default_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        let resp = server.get("/branding/configuration").await;

        resp.assert_status_ok();
        resp.assert_json(&json!({ "SplashscreenEnabled": false }));
        let body: serde_json::Value = resp.json();
        assert!(body.get("CustomCss").is_none() || body["CustomCss"].is_null());
    }

    // --- POST /branding/configuration ---

    #[tokio::test]
    async fn branding_configuration_requires_auth() {
        let (server, _ctx) = new_test_server().await.unwrap();
        server
            .post("/branding/configuration")
            .json(&json!({ "SplashscreenEnabled": false }))
            .expect_failure()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn branding_configuration_update_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // POST new branding config → 204
        server
            .post("/branding/configuration")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .json(&json!({ "CustomCss": "body{color:red}", "SplashscreenEnabled": false }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // GET and verify the CSS persisted
        let resp = server.get("/branding/configuration").await;
        resp.assert_status_ok();
        resp.assert_json_contains(&json!({ "CustomCss": "body{color:red}" }));
    }

    // --- POST /system/configuration/branding ---

    #[tokio::test]
    async fn system_configuration_branding_update_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // POST to /system/configuration/branding → 204
        server
            .post("/system/configuration/branding")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .json(&json!({ "CustomCss": "h1{font-size:2em}", "SplashscreenEnabled": false }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // GET /branding/configuration verifies the same store was updated
        let resp = server.get("/branding/configuration").await;
        resp.assert_status_ok();
        resp.assert_json_contains(&json!({ "CustomCss": "h1{font-size:2em}" }));
    }

    // --- GET /branding/css + GET /branding/css.css ---

    #[tokio::test]
    async fn branding_css_empty_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        server
            .get("/branding/css")
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn branding_css_dotcss_empty_test() {
        let (server, _ctx) = new_test_server().await.unwrap();

        server
            .get("/branding/css.css")
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn branding_css_with_css_test() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let css = "body{background:blue}";

        // POST CSS via /branding/configuration
        server
            .post("/branding/configuration")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "CustomCss": css, "SplashscreenEnabled": false }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // GET /branding/css → 200 + body equals CSS
        let css_resp = server.get("/branding/css").await;
        css_resp.assert_status_ok();
        assert_eq!(css_resp.text(), css);

        // GET /branding/css.css → 200 + same body
        let dotcss_resp = server.get("/branding/css.css").await;
        dotcss_resp.assert_status_ok();
        assert_eq!(dotcss_resp.text(), css);
    }
}
