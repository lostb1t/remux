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
use tracing_subscriber::{EnvFilter, Registry, fmt};
use url::Url;

use uuid::Uuid;

mod conversions;
mod errors;
pub mod sdks {
    pub use remux_sdks::*;
}
mod aio;
pub mod db;
#[cfg(feature = "desktop")]
pub mod embedded_static;
mod iptv;
pub mod jellyfin;
mod log_capture;
pub mod playback_session;
mod providers;
mod store;
pub mod tasks;
mod torrent;
pub mod transcode;
mod utils;
mod web_patches;
mod web_transform;
mod ws;

#[cfg(feature = "desktop")]
static EMBEDDED_DASHBOARD: std::sync::OnceLock<&'static include_dir::Dir<'static>> =
    std::sync::OnceLock::new();
#[cfg(feature = "desktop")]
static EMBEDDED_JELLYFIN_WEB: std::sync::OnceLock<&'static include_dir::Dir<'static>> =
    std::sync::OnceLock::new();

#[cfg(feature = "desktop")]
pub fn set_embedded_assets(
    dashboard: &'static include_dir::Dir<'static>,
    jellyfin_web: &'static include_dir::Dir<'static>,
) {
    EMBEDDED_DASHBOARD.set(dashboard).ok();
    EMBEDDED_JELLYFIN_WEB.set(jellyfin_web).ok();
}

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
    let (router, _ctx) = init_app_inner(config).await?;
    Ok(router)
}

pub async fn init_app_with_ctx(config: Config) -> Result<(Router, AppContext)> {
    init_app_inner(config).await
}

/// Start the HTTP server, binding to `0.0.0.0:{port}` (default 3000, or
/// `REMUX_PORT` env var).  Runs until the process exits.
pub async fn serve(config: Config) -> Result<()> {
    let port = std::env::var("REMUX_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(3000);
    let addr = format!("0.0.0.0:{port}");
    let app = MapRequestLayer::new(rewrite_request_uri)
        .layer(init_app_with_config(config).await?);
    tracing::info!("starting webserver at {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn init_app_inner(config: Config) -> Result<(Router, AppContext)> {
    log_capture::init_file(&config.log_file);
    info!("starting remux {}", env!("CARGO_PKG_VERSION"));
    info!("config: {}", serde_json::to_string_pretty(&config).unwrap());

    let conn = db::connect(&config.database_url).await?;

    info!("running database migrations…");
    db::migrate(&conn).await?;
    info!("migrations complete");
    crate::utils::init_server_id(&conn).await?;
    db::ensure_collection_folder(&conn).await?;

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
        store: store::Store::new(100000),
        sessions: playback_session::PlaybackSessionManager::new("transcode_sessions"),
        torrent: torrent_mgr.clone(),
        ws_tx,
    };

    // Apply saved P2P speed limits on startup.
    {
        let cfg = db::Settings::get_config(&conn).await?;
        if cfg.p2p_enabled.unwrap_or(true) {
            torrent_mgr.update_limits(
                cfg.p2p_upload_speed_kbps.unwrap_or(0),
                cfg.p2p_download_speed_kbps.unwrap_or(0),
            );
        }
    }

    // Kill idle sessions after 60 seconds of no activity (matches Jellyfin's HLS timeout).
    ctx.sessions.clone().spawn_cleanup_task(
        std::time::Duration::from_secs(30),
        std::time::Duration::from_secs(60),
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

    #[cfg(feature = "desktop")]
    let router = {
        use embedded_static::EmbeddedDir;
        let dashboard_dir = EMBEDDED_DASHBOARD
            .get()
            .expect("embedded dashboard not set");
        let jellyfin_web_dir = EMBEDDED_JELLYFIN_WEB
            .get()
            .expect("embedded jellyfin-web not set");
        Router::new()
            .route("/websocket", get(ws::ws_handler))
            .route("/socket", get(ws::ws_handler))
            .merge(collect_routes())
            .nest_service(
                "/admin",
                EmbeddedDir {
                    dir: dashboard_dir,
                    spa_fallback: true,
                },
            )
            .with_state(state)
            .layer(on_error(|err| {
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
            }))
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .layer(cors)
            .fallback_service(web_transform::TransformLayer::new().layer(EmbeddedDir {
                dir: jellyfin_web_dir,
                spa_fallback: false,
            }))
    };

    #[cfg(not(feature = "desktop"))]
    let router = {
        let dashboard_index = format!("{}/index.html", ctx.config.dashboard_path);
        Router::new()
            .route("/websocket", get(ws::ws_handler))
            .route("/socket", get(ws::ws_handler))
            .merge(collect_routes())
            .nest_service(
                "/admin",
                ServeDir::new(&ctx.config.dashboard_path)
                    .fallback(ServeFile::new(dashboard_index)),
            )
            .with_state(state)
            .layer(on_error(|err| {
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
            }))
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .layer(cors)
            .fallback_service(
                web_transform::TransformLayer::new()
                    .layer(ServeDir::new(ctx.config.web_path.clone())),
            )
    };

    Ok((router, ctx))
}

#[derive(Clone)]
pub struct AppContext {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub store: store::Store,
    pub sessions: playback_session::PlaybackSessionManager,
    pub torrent: Arc<torrent::TorrentManager>,
    pub ws_tx: tokio::sync::broadcast::Sender<ws::WsEvent>,
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: AppContext,
    pub tasks: tasks::TaskService,
}

#[cfg(not(feature = "desktop"))]
fn default_web_path() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("jellyfin-web"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/jellyfin-web".to_string())
}

#[cfg(not(feature = "desktop"))]
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

fn default_log_file() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("logs").join("remux.jsonl"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/logs/remux.jsonl".to_string())
}

fn default_torrent_data_dir() -> String {
    dirs::data_dir()
        .map(|d| d.join("remux").join("torrents"))
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/data/torrents".to_string())
}

const TORRENT_HTTP_PORT: u16 = 9876;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[cfg(not(feature = "desktop"))]
    #[serde(default = "default_web_path")]
    pub web_path: String,
    #[cfg(not(feature = "desktop"))]
    #[serde(default = "default_dashboard_path")]
    pub dashboard_path: String,
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_log_file")]
    pub log_file: String,
    #[serde(default = "default_torrent_data_dir")]
    pub torrent_data_dir: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            #[cfg(not(feature = "desktop"))]
            web_path: default_web_path(),
            #[cfg(not(feature = "desktop"))]
            dashboard_path: default_dashboard_path(),
            database_url: default_database_url(),
            log_file: default_log_file(),
            torrent_data_dir: default_torrent_data_dir(),
        }
    }
}

pub fn rewrite_request_uri<B>(mut req: http::Request<B>) -> http::Request<B> {
    let uri = req.uri();
    let path = uri.path().replace("/emby", "");

    if path == "/" || (path.matches('/').count() == 1 && path.matches('.').count() > 0)
    {
        return req;
    }

    let new_path = path.to_ascii_lowercase();

    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();

    let new_uri = http::Uri::builder()
        .path_and_query(format!("{}{}", new_path, query))
        .build()
        .unwrap_or_else(|_| uri.clone());

    *req.uri_mut() = new_uri;
    req
}

pub fn setup_logging() {
    let (reload_layer, log_capture, _tx) = log_capture::init();

    let fmt_layer = fmt::layer()
        .with_timer(fmt::time::ChronoLocal::new("%H:%M:%S".to_string()))
        .with_target(false)
        .with_line_number(false)
        .with_file(false)
        .compact();

    Registry::default()
        .with(reload_layer)
        .with(fmt_layer)
        .with(log_capture)
        .try_init()
        .ok(); // try_init + ok() so tests don't panic on repeated calls
}

async fn handle_404(uri: axum::http::Uri) -> impl IntoResponse {
    debug!("404 - Not Found: {}", uri);
    (StatusCode::NOT_FOUND, "Not Found")
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
