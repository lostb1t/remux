//#![feature(duration_constructors)]
#![allow(warnings)]

#[cfg(test)]
mod test;

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
//use tracing_log::LogTracer;
//use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use itertools::Itertools;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};
use url::Url;

use uuid::Uuid;



//mod auth;
mod conversions;
mod errors;
pub use shared::sdks;
mod store;
mod utils;
//mod user;
mod aio;
mod db;
mod iptv;
mod jellyfin;
mod log_capture;
mod providers;
mod playback_session;
mod tasks;
mod transcode;
mod web_patches;
mod web_transform;
mod ws;

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

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();
    let app =
        tower::util::MapRequestLayer::new(rewrite_request_uri).layer(init_app().await?);
    tracing::info!("starting webserver at 0.0.0.0:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

async fn init_app() -> Result<Router> {
    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());

    let config: Config = config::Config::builder()
        .add_source(config::File::with_name(&cfg).required(false))
        .add_source(config::Environment::default())
        .build()?
        .try_deserialize()?;

    init_app_with_config(config).await
}

pub async fn init_app_with_config(config: Config) -> Result<Router> {
    let (router, _ctx) = init_app_inner(config).await?;
    Ok(router)
}

/// Test-only variant: returns the router AND the `AppContext` (which carries
/// the `SqlitePool`) so tests can insert fixture data into the same DB the
/// server uses.
#[cfg(test)]
pub async fn init_app_with_ctx(config: Config) -> Result<(Router, AppContext)> {
    init_app_inner(config).await
}

async fn init_app_inner(config: Config) -> Result<(Router, AppContext)> {
    gstreamer::init().context("Failed to initialize GStreamer")?;
    log_capture::init_file(&config.log_file);
    debug!("config: {}", serde_json::to_string_pretty(&config).unwrap());

    let conn = db::connect(&config.db_url).await?;

    db::migrate(&conn).await?;
    crate::utils::init_server_id(&conn).await?;
    db::ensure_collection_folder(&conn).await?;

    // FOR TWSTING ONLY
    // db::checkpoint_db(&conn).await;

    let (ws_tx, _) = tokio::sync::broadcast::channel(128);

    let ctx = AppContext {
        config,
        db: conn.clone(),
        store: store::Store::new(100000),
        transcode: transcode::session::TranscodeSessionManager::new(
            "transcode_sessions",
        ),
        ws_tx,
    };

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

    let dashboard_index = format!("{}/index.html", ctx.config.dashboard_path);
    let router = Router::new()
        .route("/websocket", get(ws::ws_handler))
        .merge(collect_routes())
        .nest_service(
            "/admin",
            ServeDir::new(&ctx.config.dashboard_path)
                .fallback(ServeFile::new(dashboard_index)),
        )
        .with_state(state)
        .layer(on_error(|err| {
            tracing::error!(
                status = %err.status(),
                title = %err.title(),
                detail = %err.detail(),
                "api error"
            );
        }))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
        .fallback_service(
            web_transform::TransformLayer::new()
                .layer(ServeDir::new(ctx.config.web_path.clone())),
        );
    Ok((router, ctx))
}

#[derive(Clone)]
pub struct AppContext {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub store: store::Store,
    pub transcode: transcode::session::TranscodeSessionManager,
    pub ws_tx: tokio::sync::broadcast::Sender<ws::WsEvent>,
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: AppContext,
    pub tasks: tasks::TaskService,
}

fn default_web_path() -> String {
    "/app/jellyfin-web".to_string()
}

fn default_dashboard_path() -> String {
    "/app/dashboard".to_string()
}

fn default_db_url() -> String {
    "sqlite:///data/db.sqlite?mode=rwc".to_string()
}

fn default_log_file() -> String {
    "/data/logs/remux.jsonl".to_string()
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_web_path")]
    pub web_path: String,
    #[serde(default = "default_dashboard_path")]
    pub dashboard_path: String,
    #[serde(default = "default_db_url")]
    pub db_url: String,
    #[serde(default = "default_log_file")]
    pub log_file: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            web_path: default_web_path(),
            dashboard_path: default_dashboard_path(),
            db_url: default_db_url(),
            log_file: default_log_file(),
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
