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

use remux_utils::Store;

mod conversions;
mod errors;
pub mod sdks {
    pub use remux_sdks::*;
}
mod aio;
pub mod api;
pub mod db;
#[cfg(feature = "desktop")]
pub mod embedded_static;
mod iptv;
pub mod localization;
pub mod playback_session;
mod providers;
pub mod tasks;
mod torrent;
pub mod transcode;
mod utils;
mod web_client;
mod web_patches;
mod web_transform;
mod ws;

/// Paths to web assets served from the filesystem (non-desktop builds).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct FilesystemPaths {
    #[serde(default = "default_web_path")]
    pub web_path: String,
    #[serde(default = "default_anfiteatro_web_path")]
    pub anfiteatro_web_path: String,
    #[serde(default = "default_dashboard_path")]
    pub dashboard_path: String,
}

impl Default for FilesystemPaths {
    fn default() -> Self {
        Self {
            web_path: default_web_path(),
            anfiteatro_web_path: default_anfiteatro_web_path(),
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
    let web_client =
        WebClientService::from_filesystem(&paths.web_path, &paths.anfiteatro_web_path);
    let (router, _ctx) = init_app(config, Some(paths), admin, web_client).await?;
    Ok(router)
}

pub async fn init_app_with_ctx(config: Config) -> Result<(Router, AppContext)> {
    let paths = FilesystemPaths::default();
    let admin = admin_from_filesystem(&paths.dashboard_path.clone());
    let web_client =
        WebClientService::from_filesystem(&paths.web_path, &paths.anfiteatro_web_path);
    init_app(config, Some(paths), admin, web_client).await
}

/// Start the HTTP server with web assets served from the filesystem.
/// Binds to `0.0.0.0:{port}` (default 3000, or `PORT` env var).
pub async fn serve(config: Config, paths: FilesystemPaths) -> Result<()> {
    let admin = admin_from_filesystem(&paths.dashboard_path.clone());
    let web_client =
        WebClientService::from_filesystem(&paths.web_path, &paths.anfiteatro_web_path);
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

    let conn = db::connect(&config.database_url).await?;

    info!("running database migrations…");
    db::migrate(&conn).await?;
    info!("migrations complete");
    crate::utils::init_server_id(&conn).await?;
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
            std::path::PathBuf::from(&config.torrent_data_dir),
            TORRENT_HTTP_PORT,
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
        search: Arc::new(providers::SearchServiceManager::default()),
        streams: Arc::new(providers::StreamServiceManager::default()),
        meta: Arc::new(providers::MetaProviderService::default()),
        lyrics: Arc::new(providers::LyricService::default()),
        catalogs: Arc::new(providers::CatalogProviderManager::default()),
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

    let task_service = tasks::TaskService::new(ctx.clone()).await?;

    // Register per-catalog import tasks for existing enabled catalogs.
    let enabled_catalogs = db::Media::get_by_filter(
        &conn,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?;
    for cat in enabled_catalogs.records {
        task_service
            .register_task(std::sync::Arc::new(tasks::CatalogItemImportTask::new(
                cat.id, &cat.title,
            )))
            .await?;
    }

    let _ = task_service.run_task("EnsureAnfiteatro").await;
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
    pub search: Arc<providers::SearchServiceManager>,
    pub streams: Arc<providers::StreamServiceManager>,
    pub meta: Arc<providers::MetaProviderService>,
    pub lyrics: Arc<providers::LyricService>,
    pub catalogs: Arc<providers::CatalogProviderManager>,
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: AppContext,
    pub tasks: tasks::TaskService,
}

fn default_web_path() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("jellyfin-web"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/jellyfin-web".to_string())
}

fn default_anfiteatro_web_path() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("anfiteatro-web"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/anfiteatro-web".to_string())
}

fn default_dashboard_path() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("dashboard"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/dashboard".to_string())
}

fn default_database_url() -> String {
    let path = dirs::data_dir()
        .map(|d| d.join("remux").join("db.sqlite"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/db.sqlite".to_string());
    format!("sqlite://{}?mode=rwc", path)
}

fn default_torrent_data_dir() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("torrents"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/torrents".to_string())
}

fn default_port() -> u16 {
    3000
}

const TORRENT_HTTP_PORT: u16 = 9876;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_torrent_data_dir")]
    pub torrent_data_dir: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: default_database_url(),
            torrent_data_dir: default_torrent_data_dir(),
            port: default_port(),
        }
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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,librqbit_dht=warn,hyper=warn,sqlx=warn")
    });

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
    if let Some(cause) = err.error() {
        tracing::error!(
            status = %err.status(),
            title = %err.title(),
            detail = %err.detail(),
            cause = %format!("{:#}", cause),
            "api error"
        );
    } else {
        tracing::error!(
            status = %err.status(),
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
