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
use config::Config;
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
use tower_http::services::ServeDir;
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

//#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
//pub use ez_ffmpeg_arm as ez_ffmpeg;

//#[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
//pub use ez_ffmpeg_upstream as ez_ffmpeg;

//mod auth;
mod conversions;
mod errors;
mod sdks;
mod store;
mod utils;
//mod user;
mod aio;
mod db;
mod jellyfin;
mod meta_provider;
mod playback_session;
mod tasks;
mod transcode;
mod web_patches;
mod web_transform;

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

    let settings: Settings = config::Config::builder()
        // .set_default("server.host", "127.0.0.1")?
        .add_source(config::File::with_name(&cfg))
        .build()?
        .try_deserialize()?;

    debug!(
        "config: {}",
        serde_json::to_string_pretty(&settings).unwrap()
    );

    let conn = db::connect(
        std::env::var("DATABASE_URL")
            .as_deref()
            .unwrap_or("sqlite:///data/db.sqlite?mode=rwc"),
    )
    .await?;

    db::migrate(&conn).await?;

    db::ensure_collection_folder(&conn).await?;

    // FOR TWSTING ONLY
    // db::checkpoint_db(&conn).await;

    // users
    for u in settings.users.clone() {
        let mut user = db::User {
            id: utils::get_stable_uuid(u.key),
            username: u.username,
            is_admin: u.is_admin,
            password_hash: db::User::hash_password(&u.password)?,
            ..Default::default()
        };

       // user.save_by_username(&conn).await?;
    }

    // libraries
    let libs_titles = db::Media::get_by_filter(
        &conn,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|m| m.title)
    .collect::<Vec<String>>();

    for u in settings.libraries.clone() {
        if libs_titles.contains(&u.name) {
            continue;
        }

        let mut media = db::Media {
            title: u.name,
            kind: db::MediaKind::Catalog,
            //aio_id: u.id,
            catalog_media_kind: Some(u.media_kind),
            catalog_kind: Some(db::CatalogKind::Smart),
            promoted: 1,
            ..Default::default()
        };

       // media.save(&conn).await?;
    }

    let ctx = AppContext {
        config: settings.clone(),
        db: conn.clone(),
        aio: aio::AioService::from_url(&settings.aio_url)?,
        store: store::Store::new(100000),
        transcode: transcode::session::TranscodeSessionManager::new("transcode_sessions"),
    };

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

    Ok(Router::new()
        .merge(collect_routes())
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
            web_transform::TransformLayer::new().layer(ServeDir::new(settings.web_path)),
        ))
}

#[derive(Clone)]
pub struct AppContext {
    pub config: Settings,
    pub db: sqlx::SqlitePool,
    pub aio: aio::AioService,
    pub store: store::Store,
    pub transcode: transcode::session::TranscodeSessionManager,
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: AppContext,
    pub tasks: tasks::TaskService,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub key: String,
    pub username: String,
    pub password: String,
    pub is_admin: bool,
    //pub aio_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    pub media_kind: db::MediaKind,
}

fn default_web_path() -> String {
    "/app/jellyfin-web".to_string()
}

fn default_catalog_max_items() -> usize {
    100
}

fn default_users() -> Vec<UserConfig> {
    Vec::new()
}

fn default_libraries() -> Vec<Library> {
    vec![
        Library {
            name: "Movies".to_string(),
            media_kind: db::MediaKind::Movie,
        },
        Library {
            name: "Series".to_string(),
            media_kind: db::MediaKind::Series,
        },
    ]
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Settings {
    #[serde(deserialize_with = "clean_aio_url")]
    pub aio_url: String,
    #[serde(default = "default_web_path")]
    pub web_path: String,
    #[serde(default = "default_catalog_max_items")]
    pub catalog_max_items: usize,
    #[serde(default = "default_users")]
    pub users: Vec<UserConfig>,
    #[serde(default = "default_libraries")]
    pub libraries: Vec<Library>,
    // we dont support folders
    //#[serde(default = "default_collection_id")]
    //pub collection_id: String,
}

fn clean_aio_url<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let url = String::deserialize(deserializer)?;
    let cleaned = clean_aio_url_str(&url);
    Ok(cleaned.to_string())
}

fn clean_aio_url_str(url: &str) -> &str {
    url.trim_end_matches('/')
        .strip_suffix("manifest.json")
        .unwrap_or(url)
        .trim_end_matches('/')
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
    let filter_layer = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,sqlx=warn"));

    let fmt_layer = fmt::layer()
        .with_timer(fmt::time::ChronoLocal::new("%H:%M:%S".to_string()))
        .with_target(false)
        .with_line_number(false)
        .with_file(false)
        .compact();

    Registry::default()
        .with(filter_layer)
        .with(fmt_layer)
        .init();
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
mod integration_test {

    use super::*;
    use axum_test::TestServer;

    pub async fn new_test_server() -> Result<TestServer> {
        let app = init_app().await?;

        Ok(
            TestServer::builder()
                .save_cookies()
                //.authorization()
                .expect_success_by_default()
                .mock_transport()
                .build(app)?, // .authorization("password12345")
        )
    }

    pub async fn apply_auth(mut server: TestServer) -> TestServer {
        server.add_header("x-custom-for-all", "common-value");
        server
    }
}
