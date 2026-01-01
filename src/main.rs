//#![feature(duration_constructors)]
#![allow(warnings)]
// #[macro_use]
// extern crate serde_derive;
// extern crate serde_alias;

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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
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
use tracing::instrument;
use tracing::warn;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
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
mod jellyfin;
mod aio;
mod db;
use crate::db as database;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    setup_logging();

    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());

    let settings: Settings = config::Config::builder()
        .add_source(config::File::with_name(&cfg))
        .build()?
        .try_deserialize()?;

    tracing::info!("config: {:?}", settings);

    let db = database::connect(
        std::env::var("DATABASE_URL")
            .as_deref()
            .unwrap_or("sqlite:///data/db.sqlite"),
    )
    .await?;

    database::migrate(&db).await?;

    for u in settings.users.clone() {
      let mut user = db::User {
        id: u.stable_id_from_key(),
        username: u.username,
        aio_url: u.aio_url,
        password_hash: db::User::hash_password(&u.password)?
      };

      user.save(&db).await?;
    }

    let state = AppState {
        config: settings.clone(),
        db: db,
       // item_store: jellyfin::BaseItemStore::new(25000)
    };

    // spawn_background_tasks(state.clone()).await?;
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any) // or list them explicitly:
        .expose_headers(Any);

    let app = tower::util::MapRequestLayer::new(rewrite_request_uri)
        .layer(
            Router::new()
                .merge(jellyfin::api::routes())
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
                .fallback_service(ServeDir::new(settings.web_path)),
        )
        .into_make_service();

    tracing::info!("starting webserver at 0.0.0.0:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Settings,
    pub db: SqlitePool,
   // pub item_store: jellyfin::BaseItemStore
}

pub fn virtual_folders(
    manifest: &sdks::aio::Manifest,
) -> Vec<jellyfin::BaseItemDto> {
    let mut vf = vec![jellyfin::BaseItemDto {
        name: Some("Collections".to_string()),
        //id: "collections".to_string(),
        id: utils::MediaId::new(
            "collections".into(),
            jellyfin::MediaType::CollectionFolder,
            None,
        ),
        //parent_id: Some("test".to_string()),
        //type_: Some(jellyfin::MediaType::CollectionFolder),
        collection_type: Some(jellyfin::CollectionType::Boxsets),
        is_folder: Some(true),
        ..Default::default()
    }];
    vf.extend(
        manifest
            .catalogs
            .iter()
            // basicly, use catalogs that have show on home enabled
            .filter(|x| x.extra.iter().any(|e| e.name == "genre" && !e.is_required))
            .map(|x| jellyfin::BaseItemDto {
                name: Some(x.name.clone()),
                id: utils::MediaId::new(
                    x.id.clone(),
                    jellyfin::MediaType::CollectionFolder,
                    None,
                ),
                // none means mixed
                //collection_type: None,
                //type_: Some(jellyfin::MediaType::CollectionFolder),
                collection_type: {
                  match x.kind.as_str() {
                  "series" => Some(jellyfin::CollectionType::Tvshows),
                  _ => Some(jellyfin::CollectionType::Movies),
                }
                },
                is_folder: Some(true),
                //collection_type:
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    );
    vf
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub key: String,
    pub username: String,
    pub password: String,
    pub aio_url: String,
}

impl UserConfig {
    fn stable_id_from_key(&self) -> String {
        Uuid::new_v5(&Uuid::nil(), &self.key.clone().as_bytes()).to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    pub catalog_id: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Settings {
    #[serde(default = "default_web_path")]
    pub web_path: String,
    pub users: Vec<UserConfig>,
    //  pub libraries: Vec<Library>,
}

fn default_web_path() -> String {
    "../jellyfin-web/dist".to_string()
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
    LogTracer::init().unwrap();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,sqlx=warn"));

    let subscriber = tracing_subscriber::registry().with(filter).with(
        fmt::layer()
            // .pretty()
            .with_writer(std::io::stdout),
    );

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    //set_expose_errors(true);
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
