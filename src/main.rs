//#![feature(duration_constructors)]
#![allow(warnings)]
#[macro_use]
extern crate serde_derive;
extern crate serde_alias;

use axum::response::Html;
use reqwest;

use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use axum::ServiceExt;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
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
use anyhow::Context;
use axum_anyhow::on_error;
use axum_anyhow::set_expose_errors;
use axum_anyhow::{ApiResult, ResultExt, OptionExt};
use chrono::prelude::*;
use chrono::{Duration, Utc};
use config;
use config::Config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use axum::http::request::Parts;
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
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
use uuid::Uuid;
use toasty;
use std::sync::Arc;
use axum_anyhow::ApiError;


mod api;
mod conversions;
mod errors;
mod imdb;
mod remux;
mod sdks;
mod utils;

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
    let db = toasty::Db::builder()
    //    .register::<User>()
        .register::<AuthSession>()
        .connect(
            std::env::var("DB_URL")
                .as_deref()
                .unwrap_or("sqlite::memory:"),
        )
        .await?;
        
    let state = AppState {
        config: settings.clone(),
        db: Arc::new(db)
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
    pub db: Arc<toasty::Db>,
}

pub fn virtual_folders(
    manifest: &sdks::aio::Manifest,
) -> Vec<sdks::jellyfin::BaseItemDto> {
    let mut vf = vec![sdks::jellyfin::BaseItemDto {
        name: Some("Collections".to_string()),
        //id: "collections".to_string(),
        id: utils::MediaId::new(
            "collections".into(),
            sdks::jellyfin::MediaType::CollectionFolder,
            None,
        ),
        //parent_id: Some("test".to_string()),
        type_: Some(sdks::jellyfin::MediaType::CollectionFolder),
        collection_type: Some(sdks::jellyfin::CollectionType::Boxsets),
        is_folder: Some(true),
        ..Default::default()
    }];
    vf.extend(
        manifest
            .catalogs
            .iter()
            // basicly, use catalogs that have show on home enabled
    .filter(|x| {
    x.extra.iter().any(|e| e.name == "genre" && !e.is_required)
})
            .map(|x| sdks::jellyfin::BaseItemDto {
                name: Some(x.name.clone()),
                id: utils::MediaId::new(
                    x.id.clone(),
                    sdks::jellyfin::MediaType::CollectionFolder,
                    None,
                ),
                type_: Some(sdks::jellyfin::MediaType::CollectionFolder),
                is_folder: Some(true),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    );
    vf
}


#[derive(Debug, Clone, Serialize, Deserialize, toasty::Model)]
#[serde(rename_all = "PascalCase")]
pub struct AuthSession {
    pub user_id: Uuid,
    #[key]
    pub access_token: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
}

impl AuthSession {
  
//
fn get_by_jf_auth(auth: &JellyfinAuth) -> Result<Option<Self>> {
   Ok(None)
}

//fn save(&self) -> Result<Self> {
   //if self.access_token
//}

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
  //  pub device: Option<Device>,
}

impl FromRequestParts<AppState> for AuthState {
    type Rejection = AuthError;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = state.config.users.get(0).cloned().ok_or(AuthError)?;

        Ok(AuthState { user })
    }
}

#[derive(Debug, Clone, Default)]
pub struct JellyfinAuth {
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub token: Option<String>,
}

impl JellyfinAuth {
  
fn from_str(header: &str) -> Result<Self> {
    let mut map = HashMap::new();
    let mut parts = header.splitn(2, ' ');

    let scheme = parts.next().unwrap_or("");
    let rest = match parts.next() {
        Some(r) => r,
        None => return Ok(JellyfinAuth::default()),
    };

    if !scheme.eq_ignore_ascii_case("MediaBrowser") && !scheme.eq_ignore_ascii_case("Emby") {
        return Ok(JellyfinAuth::default());
    }

    for item in rest.split(',') {
        let item = item.trim();
        let mut kv = item.splitn(2, '=');
        if let (Some(key), Some(val)) = (kv.next(), kv.next()) {
            let unquoted = val.trim().trim_matches('"').to_string();
            map.insert(key.to_string(), unquoted);
        }
    }

    Ok(Self {
            client: map.get("Client").cloned(),
            device: map.get("Device").cloned(),
            device_id: map.get("DeviceId").cloned(),
            version: map.get("Version").cloned(),
            token: map.get("Token").cloned(),
            ..Default::default()
        })
}
}

impl FromRequestParts<AppState> for JellyfinAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
let raw = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                parts
                    .headers
                    .get("X-Emby-Authorization")
                    .and_then(|v| v.to_str().ok())
            })
            .context_unauthorized("forbidden", "forbidden")?;

        Ok(JellyfinAuth::from_str(raw)?)
    }
}







impl FromRequestParts<AppState> for AuthSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState
    ) -> Result<Self, Self::Rejection> {
      


        let jfauth = JellyfinAuth::from_request_parts(parts, state).await?;
        let token = jfauth.token.as_deref().context_unauthorized("forbidden", "forbidden")?;
        let device_id = jfauth.device_id.as_deref().context_unauthorized("forbidden", "forbidden")?;

         let session = AuthSession::get_by_jf_auth(&jfauth)?
            .context_unauthorized("forbidden", "forbidden")?;

        Ok(session)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub key: String,

    #[serde(skip)]
    id: Option<Uuid>,

    pub username: String,
    pub password: String,
    pub aio_url: String,
}

impl User {
    fn stable_id_from_key(key: &str) -> Uuid {
        Uuid::new_v5(&Uuid::nil(), key.as_bytes())
    }

    pub fn get_id(&mut self) -> Uuid {
        if let Some(id) = self.id {
            return id;
        }

        let id = Self::stable_id_from_key(&self.key);
        self.id = Some(id);
        id
    }

    pub fn get_aio(&self) -> Result<sdks::RestClient> {
        let url = self
            .aio_url
            .strip_suffix("manifest.json")
            .unwrap_or(self.aio_url.as_str());

        Ok(sdks::aio::client(url)?)
    }

    pub fn get_aio_search(&self) -> Result<sdks::RestClient<sdks::BasicAuth>> {
        let mut url = Url::parse(&self.aio_url)?;

        let segments: Vec<&str> = url
            .path_segments()
            .ok_or_else(|| anyhow!("url has no path segments"))?
            .collect();

        if segments.len() < 3 {
            return Err(anyhow!(
                "invalid aio_url format: expected /stremio/<username>/<password>/..."
            ));
        }

        let username = segments[1].to_string();
        let password = segments[2].to_string();

        url.set_path("/api/v1");
        url.set_query(None);
        url.set_fragment(None);

        let search_url = url.as_str().to_string();

        Ok(sdks::aio::search_client(&search_url, username, password)?)
    }
    
    pub async fn get_stream(
    &self,
    media_type: sdks::aio::MediaType,
    id: String,
    stream_id: String,
) -> Result<sdks::aio::Stream> {
    let streams = self
        .get_aio_search()?
        .execute(&sdks::aio::Search {
            kind: media_type.into(),
            id,
            ..Default::default()
        })
        .await?;

    let stream = streams
        .data
        .results
        .into_iter()
        .find(|x| x.id() == stream_id)
        .context("no stream")?;

    Ok(stream)
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
    pub users: Vec<User>,
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
