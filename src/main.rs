//#![feature(duration_constructors)]
#![allow(warnings)]
#[macro_use]
extern crate serde_derive;
extern crate serde_alias;

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
use axum::extract::{FromRequestParts};
use chrono::prelude::*;
use chrono::{Duration, Utc};
use futures::future::BoxFuture;
use http::Uri;
use config::Config;
use futures_util::StreamExt;
use reqwest::header::LOCATION;
use serde_json::json;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
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
use anyhow::anyhow;
use anyhow::Result;
use axum_anyhow::{ApiResult, OptionExt};
use http::request::Parts;
use config;
use async_trait::async_trait;

mod api;
mod conversions;
mod errors;
mod imdb;
mod sdks;
mod utils;
mod remux;

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

    let state = AppState {
        config: settings.clone(),
      //  db: db::Database::new().await?
        //users: settings.users,
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
                .merge(api::routes())
                .with_state(state)
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
  //  pub db: db::Database,
   // pub tmdb: sdks::RestClient,
   // pub stremio: sdks::aio::StremioService,
}

#[derive(Debug)]
pub struct AuthError;

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        axum::http::StatusCode::UNAUTHORIZED.into_response()
    }
}


pub struct AuthState {
    pub user: User,
    pub device: Option<String>
}

//#[async_trait]
impl FromRequestParts<AppState> for AuthState {
    type Rejection = AuthError;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = state
            .config
            .users
            .get(0)
            .cloned()
            .ok_or(AuthError)?;

        Ok(AuthState {
            user,
            device: None,
        })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password: String,
    pub aio_url: String,
}

impl User {
   pub fn get_aio(&self) ->  Result<sdks::RestClient> {
          Ok(sdks::aio::client(&self.aio_url)?)
 } 
pub fn get_aio_search(&self) -> Result<sdks::RestClient<sdks::BasicAuth>> {
        let mut url = Url::parse(&self.aio_url)?;

        let segments: Vec<String> = url
            .path_segments()
            .ok_or_else(|| anyhow!("url has no path segments"))?
            .map(|s| s.to_string())
            .collect();

        if segments.len() < 3 {
            return Err(anyhow!(
                "invalid aio_url format: expected /stremio/<username>/<password>/..."
            ));
        }

        let username = segments[1].clone();
        let password = segments[2].clone();

        // Build https://host/api/v1/search (preserve scheme/host/port/query is dropped intentionally)
        url.set_path("/api/v1/search");
        url.set_query(None);
        url.set_fragment(None);

        let search_url = url.as_str().to_string();

        Ok(sdks::aio::search_client(&search_url, username, password)?)
    }


}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Settings {
    #[serde(default = "default_web_path")]
    pub web_path: String,
    pub users: Vec<User>,
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

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer()
        // .pretty()
        .with_writer(std::io::stdout));

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");
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
