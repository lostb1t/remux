#![allow(warnings)]

use axum::response::Html;
use reqwest;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use axum::ServiceExt;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::middleware;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::{
    Json, Router,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
};
use axum_anyhow::ApiError;
use axum_anyhow::on_error;
use axum_anyhow::set_expose_errors;
use axum_anyhow::{ApiResult, OptionExt, ResultExt};
use chrono::prelude::*;
use chrono::{Duration, Utc};
use config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use itertools::Itertools;
use remux_utils::Store;
use reqwest::header::LOCATION;
use serde::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use timed;
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing;
use tracing::debug;
use tracing::info;
use tracing::instrument;
use tracing::warn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};
use url::Url;
use uuid::Uuid;

mod conversions;
mod errors;
mod keyed_lock;
pub mod profile;
pub mod sdks {
    pub use remux_sdks::*;
}
mod addons;
pub mod api;
mod common;
pub mod db;
#[cfg(feature = "desktop")]
pub mod embedded_static;
mod iptv;
pub mod localization;
pub mod playback_session;
pub mod services;
pub mod stream;
pub mod tasks;
mod torrent;
pub mod transcode;
mod web_client;
mod web_patches;
mod web_transform;
mod ws;

/// Paths to web assets served from the filesystem (non-desktop builds).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FilesystemPaths {
    #[serde(default = "default_web_path")]
    pub web_path: String,
    #[serde(default = "default_dashboard_path")]
    pub dashboard_path: String,
}

impl Default for FilesystemPaths {
    fn default() -> Self {
        Self {
            web_path: default_web_path(),
            dashboard_path: default_dashboard_path(),
        }
    }
}

/// Opaque service type for the `/admin` static file handler.
pub type AdminService = tower::util::BoxCloneSyncService<
    axum::extract::Request,
    axum::response::Response,
    std::convert::Infallible,
>;

/// Build an `AdminService` that serves dashboard files from the filesystem.
pub fn admin_from_filesystem(dashboard_path: &str) -> AdminService {
    let index = format!("{dashboard_path}/index.html");
    tower::util::BoxCloneSyncService::new(
        web_transform::TransformLayer::new()
            .layer(ServeDir::new(dashboard_path).fallback(ServeFile::new(index))),
    )
}

pub use web_client::WebClientService;

/// Route auto-registration via `#[get("/path")]`, `#[post("/path")]`, etc.
pub struct RouteRegistration(pub fn(axum::Router<AppState>) -> axum::Router<AppState>);
inventory::collect!(RouteRegistration);

pub fn collect_routes() -> axum::Router<AppState> {
    let mut router = axum::Router::new();
    for entry in inventory::iter::<RouteRegistration> {
        router = (entry.0)(router);
    }
    router
}

pub async fn init_app_with_config(config: Config) -> Result<Router> {
    let paths = FilesystemPaths::default();
    let admin = admin_from_filesystem(&paths.dashboard_path.clone());
    let web_client = WebClientService::from_filesystem(&paths.web_path);
    let (router, _ctx) = init_app(config, Some(paths), admin, web_client).await?;
    Ok(router)
}

pub async fn init_app_with_ctx(config: Config) -> Result<(Router, AppContext)> {
    let paths = FilesystemPaths::default();
    let admin = admin_from_filesystem(&paths.dashboard_path.clone());
    let web_client = WebClientService::from_filesystem(&paths.web_path);
    init_app(config, Some(paths), admin, web_client).await
}

/// Start the HTTP server with web assets served from the filesystem.
/// Binds to `0.0.0.0:{port}` (default 3000, or `PORT` env var).
pub async fn serve(config: Config, paths: FilesystemPaths) -> Result<()> {
    let admin = admin_from_filesystem(&paths.dashboard_path.clone());
    let web_client = WebClientService::from_filesystem(&paths.web_path);
    let port = config.port;
    let (router, _) = init_app(config, Some(paths), admin, web_client).await?;
    bind_and_serve(router, port).await
}

pub async fn bind_and_serve(router: Router, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let app = MapRequestLayer::new(rewrite_request_uri).layer(router);
    tracing::info!("starting webserver at {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

pub async fn init_app(
    config: Config,
    web_paths: Option<FilesystemPaths>,
    admin: AdminService,
    web_client: WebClientService,
) -> Result<(Router, AppContext)> {
    info!("starting remux {}", env!("CARGO_PKG_VERSION"));
    info!("config: {}", serde_json::to_string_pretty(&config).unwrap());

    let conn = db::connect(
        config
            .database_url
            .as_deref()
            .expect("Config::resolve() must be called before init_app"),
    )
    .await?;

    info!("running database migrations…");
    db::migrate(&conn).await?;
    info!("migrations complete");
    crate::common::init_server_id(&conn).await?;

    // Probe hardware and persist results at startup.
    // vaapi_driver is always re-detected (regardless of auto_detect) because
    // it is a runtime property of the host, not a user preference.
    {
        let mut enc_opts = db::Settings::get_encoding_config(&conn).await?;
        if enc_opts.auto_detect_hardware_acceleration.unwrap_or(true) {
            let detected =
                crate::transcode::engine::detect_hardware_acceleration().await;
            enc_opts.hardware_acceleration_type = Some(detected);
        }
        let device = enc_opts
            .vaapi_device
            .as_deref()
            .unwrap_or("/dev/dri/renderD128");
        let driver = crate::transcode::engine::detect_vaapi_driver(device).await;
        enc_opts.vaapi_driver = Some(driver);
        db::Settings::set_encoding_config(&conn, &enc_opts).await?;
    }

    db::ensure_collection_folder(&conn).await?;

    let saved_config = db::Settings::get_config(&conn).await?;
    let default_web_client = Arc::new(tokio::sync::RwLock::new(
        web_client::normalize_web_client(saved_config.default_web_client)
            .as_str()
            .to_string(),
    ));

    let (ws_tx, _) = tokio::sync::broadcast::channel(128);

    let torrent_mgr = Arc::new(
        torrent::TorrentManager::new(
            std::path::PathBuf::from(
                config
                    .torrent_data_dir
                    .as_deref()
                    .expect("Config::resolve() must be called before init_app"),
            ),
            config.torrent_http_port,
            config.disable_dht,
            config.torrent_peer_port,
        )
        .await?,
    );

    let ctx = AppContext {
        config,
        db: conn.clone(),
        store: Store::new(100000),
        sessions: playback_session::PlaybackSessionManager::new("transcode_sessions"),
        torrent: torrent_mgr.clone(),
        ws_tx,
        default_web_client,
        web_paths,
        addons: addons::AddonService::from_db(&conn).await?,
    };

    // Apply saved P2P speed limits on startup.
    {
        if saved_config.p2p_enabled.unwrap_or(true) {
            torrent_mgr.update_limits(
                saved_config.p2p_upload_speed_kbps.unwrap_or(0),
                saved_config.p2p_download_speed_kbps.unwrap_or(0),
            );
        }
    }

    // Kill idle sessions after 30 minutes of no activity.
    // 30 min matches a "stepped away" scenario; pings keep active sessions alive indefinitely.
    ctx.sessions.clone().spawn_cleanup_task(
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(60 * 30),
    );

    db::StreamGroup::migrate_from_settings(&conn).await;

    let task_service = tasks::TaskService::new(ctx.clone()).await?;

    task_service.run_startup_tasks().await?;

    let state = AppState {
        ctx: ctx.clone(),
        tasks: task_service,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(Any);

    let base = Router::new()
        .route("/websocket", get(ws::ws_handler))
        .route("/socket", get(ws::ws_handler))
        .merge(collect_routes());

    let router = base
        .nest_service("/admin", admin)
        .with_state(state)
        .fallback_service(web_client);

    let router = router
        .layer(on_error(log_api_error))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors);

    Ok((router, ctx))
}

#[derive(Clone)]
pub struct AppContext {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub store: Store,
    pub sessions: playback_session::PlaybackSessionManager,
    pub torrent: Arc<torrent::TorrentManager>,
    pub ws_tx: tokio::sync::broadcast::Sender<ws::WsEvent>,
    pub default_web_client: Arc<tokio::sync::RwLock<String>>,
    /// Present in filesystem builds; `None` in desktop (assets are embedded).
    pub web_paths: Option<FilesystemPaths>,
    pub addons: addons::AddonService,
}

impl AppContext {
    /// Gracefully shut down background services (torrent DHT, etc.).
    /// Call this when the server is stopping to release sockets immediately.
    pub async fn shutdown(&self) {
        self.torrent.shutdown().await;
    }
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: AppContext,
    pub tasks: tasks::TaskService,
}

fn default_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .map(|d| d.join("remux"))
        .unwrap_or_else(|| std::path::PathBuf::from("/data"))
}

fn default_web_path() -> String {
    default_data_dir()
        .join("jellyfin-web")
        .to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "/data/jellyfin-web".to_string())
}

fn default_dashboard_path() -> String {
    default_data_dir()
        .join("dashboard")
        .to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "/data/dashboard".to_string())
}

fn default_port() -> u16 {
    3000
}

fn default_torrent_http_port() -> u16 {
    9876
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: std::path::PathBuf,
    /// `None` means derive from `data_dir` — call `resolve()` after loading.
    pub database_url: Option<String>,
    /// `None` means derive from `data_dir` — call `resolve()` after loading.
    pub torrent_data_dir: Option<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Explicit port for the internal torrent HTTP server.
    /// When absent the OS picks a free ephemeral port.
    #[serde(default = "default_torrent_http_port_opt")]
    pub torrent_http_port: Option<u16>,
    /// Disable the DHT gossip socket. Useful when no Torznab sources are
    /// configured or when running in a restricted network environment.
    #[serde(default)]
    pub disable_dht: bool,
    /// TCP port range for librqbit peer connections.  Announced to trackers so
    /// they return us in peer lists.  Defaults to 6881.  Does not need to be
    /// forwarded/open for outbound-only operation, but must be a real port
    /// (not 0) or many trackers will reject the announce.
    #[serde(default = "default_torrent_peer_port")]
    pub torrent_peer_port: Option<u16>,
}

fn default_torrent_http_port_opt() -> Option<u16> {
    Some(default_torrent_http_port())
}

fn default_torrent_peer_port() -> Option<u16> {
    Some(6881)
}

impl Config {
    /// Fill in `None` fields that derive from `data_dir`. Call once after loading.
    pub fn resolve(mut self) -> Self {
        if self.database_url.is_none() {
            self.database_url = Some(format!(
                "sqlite://{}?mode=rwc",
                self.data_dir.join("db.sqlite").display()
            ));
        }
        if self.torrent_data_dir.is_none() {
            self.torrent_data_dir = Some(
                self.data_dir
                    .join("torrents")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            database_url: None,
            torrent_data_dir: None,
            port: default_port(),
            torrent_http_port: default_torrent_http_port_opt(),
            disable_dht: false,
            torrent_peer_port: default_torrent_peer_port(),
        }
        .resolve()
    }
}

pub fn rewrite_request_uri<B>(mut req: http::Request<B>) -> http::Request<B> {
    let uri = req.uri();
    let mut path = uri.path().replace("/emby", "");
    if path.is_empty() {
        path = "/".to_string();
    }

    // Keep file paths case-sensitive (Linux filesystems are case-sensitive).
    // Only normalize API-style routes that don't look like files, plus known
    // API file endpoints (for example /Videos/.../Stream.vtt).
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    let is_file_like = last_segment.contains('.');
    let lower_path = path.to_ascii_lowercase();
    let api_file_like = is_file_like
        && (lower_path.starts_with("/videos/")
            || lower_path.starts_with("/audio/")
            || lower_path.starts_with("/items/")
            || lower_path.starts_with("/mediasegments/")
            || lower_path.starts_with("/sessions/"));
    let new_path = if path != "/" && (!is_file_like || api_file_like) {
        lower_path
    } else {
        path
    };

    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();

    let new_uri = http::Uri::builder()
        .path_and_query(format!("{}{}", new_path, query))
        .build()
        .unwrap_or_else(|_| uri.clone());

    *req.uri_mut() = new_uri;
    req
}

pub fn setup_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,remux=info"));

    let fmt_layer = fmt::layer()
        .with_timer(fmt::time::ChronoLocal::new("%H:%M:%S".to_string()))
        .with_target(false)
        .with_line_number(false)
        .with_file(false)
        .compact();

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()
        .ok(); // try_init + ok() so tests don't panic on repeated calls
}

async fn handle_404(uri: axum::http::Uri) -> impl IntoResponse {
    debug!("404 - Not Found: {}", uri);
    (StatusCode::NOT_FOUND, "Not Found")
}

fn log_api_error(err: &axum_anyhow::ApiError) {
    let status = err.status();
    let is_server_error = status.is_server_error();
    if let Some(cause) = err.error() {
        if is_server_error {
            tracing::error!(
                status = %status,
                title = %err.title(),
                detail = %err.detail(),
                cause = %format!("{:#}", cause),
                "api error"
            );
        } else {
            tracing::debug!(
                status = %status,
                title = %err.title(),
                detail = %err.detail(),
                cause = %format!("{:#}", cause),
                "api error"
            );
        }
    } else if is_server_error {
        tracing::error!(
            status = %status,
            title = %err.title(),
            detail = %err.detail(),
            "api error"
        );
    } else {
        tracing::debug!(
            status = %status,
            title = %err.title(),
            detail = %err.detail(),
            "api error"
        );
    }
}

async fn handle_static_404(req: Request<Body>) -> ApiResult<impl IntoResponse> {
    tracing::debug!(
        "Static 404 Not Found: {} {}",
        req.method(),
        req.uri().path()
    );
    Ok((StatusCode::NOT_FOUND, "404 - File not found"))
}

#[cfg(test)]
pub mod integration_test;
