//#![feature(duration_constructors)]
#![allow(warnings)]
#[macro_use]
extern crate serde_derive;

use axum::response::Html;
use reqwest;

use axum::body::Body;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::{
    Json, Router,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use axum::ServiceExt;
use axum::extract::Request;
use axum::middleware;
use axum::middleware::Next;
use chrono::prelude::*;
use chrono::{Duration, Utc};
use eyre::Result;
use futures::future::BoxFuture;
//use futures::stream::StreamExt;
use http::Uri;
//use itertools::Itertools;
use figment;
use futures_util::StreamExt;
use reqwest::header::LOCATION;
use sea_orm;
use sea_orm::ColumnTrait;
use sea_orm::EntityOrSelect;
use sea_orm::EntityTrait;
use sea_orm::QueryFilter;
use sea_orm::QuerySelect;
use serde_json::json;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use timed;
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_http::services::ServeDir;
use tracing;
use tracing::debug;
use tracing::instrument;
use tracing::warn;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};

mod api;
mod conversions;
mod db;
mod errors;
mod imdb;
mod sdks;
mod utils;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    setup_logging();

    let config: Config = figment::Figment::new()
        //  .merge(figmentToml::file("Cargo.toml"))
        .merge(figment::providers::Env::prefixed(""))
        .extract()?;

    let state = api::AppState {
        config: config.clone(),
        db: db::Database::new().await?,
        tmdb: sdks::core::RestClient::new("https://api.themoviedb.org/3")
        .expect("to work")
        .header("Authorization", "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiIwZDczZTBjYjkxZjM5ZTY3MGIwZWZhNjkxM2FmYmQ1OCIsIm5iZiI6MTUzMjkzOTA3My41MzcsInN1YiI6IjViNWVjYjQxMGUwYTI2MmU5MDA0NjNjMCIsInNjb3BlcyI6WyJhcGlfcmVhZCJdLCJ2ZXJzaW9uIjoxfQ.vfOGe8_35CxhjjZXdnR2iAwdOMIY0VFYMBQrLWuRqn8"),
        stremio: sdks::stremio::StremioService::new(config.addons).await.expect("proper manifest url")
    };

    // spawn_background_tasks(state.clone()).await?;

    let app = tower::util::MapRequestLayer::new(rewrite_request_uri)
        .layer(
            Router::new()
                .merge(api::routes())
                .with_state(state)
                .layer(tower_http::trace::TraceLayer::new_for_http())
                .fallback_service(ServeDir::new("../jellyfin-web/dist")),
        )
        .into_make_service();

    tracing::info!("starting webserver at 0.0.0.0:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub addons: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addons: vec![
                "https://torrentio.strem.fun/manifest.json".to_string(),
                "https://v3-cinemeta.strem.io/manifest.json".to_string(),
            ],
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = env::var("CONFIG").unwrap_or_else(|_| "config.toml".into());
        let data = fs::read_to_string(path)?;
        Ok(toml::from_str(&data)?)
    }

    pub fn load_or_create() -> Result<Self> {
        let path = env::var("CONFIG").unwrap_or_else(|_| "config.toml".into());

        if let Ok(data) = fs::read_to_string(&path) {
            return Ok(toml::from_str(&data)?);
        }

        let cfg = Config::default();
        cfg.save()?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = env::var("CONFIG").unwrap_or_else(|_| "config.toml".into());
        let data = toml::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct AddonConfig {
    url: String,
    //search_movies: bool,
    //search_shows: bool,
    //meta_shows: bool,
    //meta_movies: bool
}

async fn spawn_background_tasks(state: api::AppState) -> Result<()> {
    tokio::spawn({
        let cstate = state.clone();
        async move {
            //let stream = imdb::TitleBasics::stream().await.unwrap();
            let stream = utils::FileStream::<imdb::TitleBasics>::from_url(
                "https://datasets.imdbws.com/title.basics.tsv.gz",
            )
            .await
            .unwrap();
            let chunk_size = 500;
            let mut imported = 0;

            let mut chunks = stream.chunks(chunk_size);

            while let Some(chunk) = chunks.next().await {
                let items: Vec<db::media::Model> = chunk
                    .into_iter()
                    .filter_map(|x| x.ok().and_then(|v| v.try_into().ok()))
                    .collect();

                if let Err(e) = db::media::Model::bulk_upsert(items, &cstate.db).await {
                    tracing::error!("bulk_upsert failed: {:?}", e);
                    continue;
                }
                imported += chunk_size;
                tracing::info!("Imported {} items so far", imported);
            }
        }
    });

    Ok(())
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

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stdout));

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");
}

async fn handle_404(uri: axum::http::Uri) -> impl IntoResponse {
    debug!("404 - Not Found: {}", uri);
    (StatusCode::NOT_FOUND, "Not Found")
}

async fn handle_static_404(req: Request<Body>) -> Result<impl IntoResponse> {
    tracing::debug!(
        "Static 404 Not Found: {} {}",
        req.method(),
        req.uri().path()
    );
    Ok((StatusCode::NOT_FOUND, "404 - File not found"))
}
