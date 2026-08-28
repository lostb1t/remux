#![allow(warnings)]

use axum::response::Html;
use reqwest;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use axum::{
    Json, Router, ServiceExt,
    body::Body,
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    middleware,
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_anyhow::{ApiError, ApiResult, on_error, set_expose_errors};
pub mod result_ext;
use chrono::{Duration, Utc, prelude::*};
use config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use itertools::Itertools;
use remux_utils::Store;
use reqwest::header::LOCATION;
pub use result_ext::{IntoApiError, OptionExt, ResultExt};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;
use std::{self, collections::HashMap, env, fs, path::Path, sync::Arc};
use timed;
use tower::{Layer, util::MapRequestLayer};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};
use tracing::{self, debug, error, info, instrument, warn};
use tracing_subscriber::{
    EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};
use url::Url;
use uuid::Uuid;

mod conversions;
pub mod device_profile;
mod errors;
mod keyed_lock;
pub mod sdks {
    pub use remux_sdks::*;
}
mod addons;
pub mod api;
mod common;
pub mod jellyfin_client;
pub use common::stable_media_uuid;
pub mod db;
#[cfg(feature = "desktop")]
pub mod embedded_static;
pub mod intro;
mod iptv;
pub mod localization;
pub mod playback;
pub mod playback_session;
pub mod services;
pub mod stream;
pub mod tasks;
mod torrent;
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

impl FilesystemPaths {
    pub fn load_from_env() -> Self {
        let mut paths = Self::default();
        if let Ok(v) = std::env::var("WEB_PATH") {
            paths.web_path = v;
        }
        if let Ok(v) = std::env::var("DASHBOARD_PATH") {
            paths.dashboard_path = v;
        }
        paths
    }
}

/// Opaque service type for the `/admin` static file handler.
pub type AdminService = tower::util::BoxCloneSyncService<
    axum::extract::Request,
    axum::response::Response,
    std::convert::Infallible,
>;

/// Build an `AdminService` that serves dashboard files from the filesystem.
/// Does not inject user custom JS (admin panel is remux's own UI).
pub fn admin_from_filesystem(dashboard_path: &str) -> AdminService {
    let index = format!("{dashboard_path}/index.html");
    tower::util::BoxCloneSyncService::new(
        web_transform::TransformLayer::new(None)
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
    let admin = admin_from_filesystem(
        &paths
            .dashboard_path
            .clone(),
    );
    let web_path = paths
        .web_path
        .clone();
    let (router, _ctx) = init_app(config, Some(paths), admin, move |pool| {
        WebClientService::from_filesystem(&web_path, pool)
    })
    .await?;
    Ok(router)
}

pub async fn init_app_with_ctx(config: Config) -> Result<(Router, AppContext)> {
    let paths = FilesystemPaths::default();
    let admin = admin_from_filesystem(
        &paths
            .dashboard_path
            .clone(),
    );
    let web_path = paths
        .web_path
        .clone();
    init_app(config, Some(paths), admin, move |pool| {
        WebClientService::from_filesystem(&web_path, pool)
    })
    .await
}

/// Start the HTTP server with web assets served from the filesystem.
/// Binds to `0.0.0.0:{port}` (default 3000, or `PORT` env var).
pub async fn serve(config: Config, paths: FilesystemPaths) -> Result<()> {
    let admin = admin_from_filesystem(
        &paths
            .dashboard_path
            .clone(),
    );
    let web_path = paths
        .web_path
        .clone();
    let port = config.port;
    let (router, _) = init_app(config, Some(paths), admin, move |pool| {
        WebClientService::from_filesystem(&web_path, pool)
    })
    .await?;
    bind_and_serve(router, port).await
}

pub async fn bind_and_serve(router: Router, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let app = MapRequestLayer::new(rewrite_request_uri).layer(router);
    info!("starting webserver at {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[cfg(unix)]
const TARGET_OPEN_FILE_LIMIT: libc::rlim_t = 8192;

#[cfg(unix)]
fn desired_open_file_limit(
    current_soft: libc::rlim_t,
    current_hard: libc::rlim_t,
) -> Option<libc::rlim_t> {
    let desired = TARGET_OPEN_FILE_LIMIT.min(current_hard);
    (current_soft < desired).then_some(desired)
}

#[cfg(unix)]
fn raise_open_file_limit() {
    // Torrent peers, tracker sockets, HTTP clients, and transcoders all share
    // the process descriptor table. Some desktop launch environments provide a
    // soft limit too small for concurrent playback, so raise only the soft
    // limit while preserving the administrator-controlled hard limit.
    unsafe {
        let mut limits = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, limits.as_mut_ptr()) != 0 {
            warn!(
                error = %std::io::Error::last_os_error(),
                "failed to read open-file limit"
            );
            return;
        }

        let mut limits = limits.assume_init();
        let Some(desired) = desired_open_file_limit(limits.rlim_cur, limits.rlim_max)
        else {
            return;
        };
        let previous = limits.rlim_cur;
        limits.rlim_cur = desired;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limits) != 0 {
            warn!(
                previous,
                desired,
                hard = limits.rlim_max,
                error = %std::io::Error::last_os_error(),
                "failed to raise open-file limit"
            );
        } else {
            info!(
                previous,
                desired,
                hard = limits.rlim_max,
                "raised open-file limit"
            );
        }
    }
}

#[cfg(not(unix))]
fn raise_open_file_limit() {}

pub async fn init_app(
    config: Config,
    web_paths: Option<FilesystemPaths>,
    admin: AdminService,
    make_web_client: impl FnOnce(sqlx::SqlitePool) -> WebClientService,
) -> Result<(Router, AppContext)> {
    raise_open_file_limit();
    info!("starting remux {}", env!("CARGO_PKG_VERSION"));
    info!("config: {}", serde_json::to_string_pretty(&config).unwrap());

    let conn = db::connect(
        config
            .database_url
            .as_deref()
            .expect("Config::resolve() must be called before init_app"),
        config.slow_query_threshold_ms,
    )
    .await?;

    info!("Running database migrations. Do not interrupt!");
    db::migrate(&conn).await?;
    info!("migrations complete");

    // Checkpoint the WAL before accepting any requests. At this point no
    // other readers exist, so TRUNCATE is guaranteed to succeed and the WAL
    // is cleared to zero — preventing large WALs left over from previous
    // write-heavy tasks (metadata refresh, library scan) from slowing down
    // the first queries after a restart.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&conn)
        .await
        .ok();
    crate::db::Settings::init_server_id(&conn).await?;

    // Probe hardware and persist results at startup.
    // vaapi_driver is always re-detected (regardless of auto_detect) because
    // it is a runtime property of the host, not a user preference.
    {
        let mut enc_opts = db::Settings::get_encoding_config(&conn).await?;
        if enc_opts
            .auto_detect_hardware_acceleration
            .unwrap_or(true)
        {
            let detected =
                crate::playback::engine::detect_hardware_acceleration().await;
            enc_opts.hardware_acceleration_type = Some(detected);
        }
        let device = enc_opts
            .vaapi_device
            .as_deref()
            .unwrap_or("/dev/dri/renderD128");
        let driver = crate::playback::engine::detect_vaapi_driver(device).await;
        enc_opts.vaapi_driver = Some(driver);
        db::Settings::set_encoding_config(&conn, &enc_opts).await?;
    }

    let saved_config = db::Settings::get_config(&conn).await?;

    let p2p_enabled = saved_config
        .p2p_enabled
        .unwrap_or(true);
    let torrent_mgr = if p2p_enabled {
        let mgr = torrent::TorrentManager::from_config(&config).await?;
        mgr.update_limits(
            saved_config
                .p2p_upload_speed_kbps
                .unwrap_or(0),
            saved_config
                .p2p_download_speed_kbps
                .unwrap_or(0),
        );
        Some(mgr)
    } else {
        None
    };

    let web_client = make_web_client(conn.clone());

    let addons = addons::AddonService::from_db(&conn, &config).await?;
    let transcode_sessions_dir = resolve_transcode_dir(&config.data_dir);
    let ctx = AppContext {
        config,
        db: conn.clone(),
        store: Store::new_weighted(128 * 1024 * 1024),
        sessions: playback_session::PlaybackSessionManager::new(transcode_sessions_dir),
        torrent: Arc::new(tokio::sync::RwLock::new(torrent_mgr.map(Arc::new))),
        ws_tx: tokio::sync::broadcast::channel(128).0,
        default_web_client: Arc::new(tokio::sync::RwLock::new(
            web_client::normalize_web_client(saved_config.default_web_client)
                .as_str()
                .to_string(),
        )),
        web_paths,
        addons,
        started_at: Utc::now(),
    };

    // Sync intro items at startup (best-effort; errors are logged not fatal).
    if let Err(e) = intro::sync_intros(&ctx).await {
        warn!(err = ?e, "intro sync failed at startup");
    }

    // Kill idle sessions after 30 minutes of no activity.
    // 30 min matches a "stepped away" scenario; pings keep active sessions alive indefinitely.
    ctx.sessions
        .clone()
        .spawn_cleanup_task(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(60 * 15),
        );

    db::StreamGroup::migrate_from_settings(&conn).await;

    let task_service = tasks::TaskService::new(ctx.clone()).await?;

    task_service
        .start()
        .await?;
    task_service
        .run_startup_tasks()
        .await?;

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
        .route(
            "/",
            get(|uri: axum::http::Uri| async move {
                let q = uri
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default();
                Redirect::permanent(&format!("/web/{q}"))
            }),
        )
        .route("/serviceworker.js", get(web_client::root_serviceworker))
        .merge(collect_routes());

    let router = base
        .nest_service("/admin", admin)
        .nest_service("/web", web_client.clone())
        .nest_service("/jellyfin", web_client)
        .with_state(state);

    let router = router
        .layer(on_error(log_api_error))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|_request: &axum::http::Request<axum::body::Body>| {
                    // let uri = _request.uri();
                    // let path = uri.path();
                    // let uri_str = match uri.query() {
                    //     Some(q) => format!("{path}?{q}"),
                    //     None => path.to_string(),
                    // };
                    tracing::info_span!("request", user = tracing::field::Empty)
                })
                .on_request(|request: &axum::http::Request<axum::body::Body>, _span: &tracing::Span| {
                    let uri = request.uri();
                    let path = uri.path();
                    let full = match uri.query() {
                        Some(q) => format!("{path}?{q}"),
                        None => path.to_string(),
                    };
                    debug!(target: "remux_server::request", method = %request.method(), uri = %full, "→");
                })
                .on_response(|response: &axum::http::Response<axum::body::Body>, latency: std::time::Duration, _span: &tracing::Span| {
                    debug!(target: "remux_server::request", status = %response.status().as_u16(), latency_ms = %latency.as_millis(), "←");
                })
                .on_failure(()),
        )
        .layer(cors);

    Ok((router, ctx))
}

#[derive(Clone)]
pub struct AppContext {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub store: Store,
    pub sessions: playback_session::PlaybackSessionManager,
    pub torrent: Arc<tokio::sync::RwLock<Option<Arc<torrent::TorrentManager>>>>,
    pub ws_tx: tokio::sync::broadcast::Sender<ws::WsEvent>,
    pub default_web_client: Arc<tokio::sync::RwLock<String>>,
    /// Present in filesystem builds; `None` in desktop (assets are embedded).
    pub web_paths: Option<FilesystemPaths>,
    pub addons: addons::AddonService,
    /// When this server process started.
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl AppContext {
    /// Gracefully shut down background services (torrent DHT, etc.).
    /// Call this when the server is stopping to release sockets immediately.
    pub async fn shutdown(&self) {
        let old = self
            .torrent
            .write()
            .await
            .take();
        if let Some(mgr) = old {
            mgr.shutdown()
                .await;
        }
    }

    pub async fn set_p2p_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let current = self
            .torrent
            .read()
            .await
            .clone();
        match (enabled, current.is_some()) {
            (true, false) => {
                let mgr =
                    Arc::new(torrent::TorrentManager::from_config(&self.config).await?);
                *self
                    .torrent
                    .write()
                    .await = Some(mgr);
            }
            (false, true) => {
                let old = self
                    .torrent
                    .write()
                    .await
                    .take();
                if let Some(mgr) = old {
                    mgr.shutdown()
                        .await;
                }
            }
            _ => {}
        }
        Ok(())
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
    /// Log queries that exceed this threshold in milliseconds. Defaults to 10 000 ms.
    #[serde(default = "default_slow_query_threshold_ms")]
    pub slow_query_threshold_ms: u64,
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
    /// Path to the bgutil-pot binary used by yt-dlp for YouTube POT token generation.
    #[serde(default = "default_bgutil_script_path")]
    pub bgutil_script_path: std::path::PathBuf,
    /// Base URL for the TMDB API. Overridable for testing.
    #[serde(default = "default_tmdb_base_url")]
    pub tmdb_base_url: String,
    /// Base URL for the Trakt API. Overridable for testing.
    #[serde(default = "default_trakt_base_url")]
    pub trakt_base_url: String,
    /// Base URL for remuxdb. When set, probe results are submitted after each live probe.
    #[serde(default = "default_remuxdb_url")]
    pub remuxdb_url: Option<String>,
    #[serde(default = "default_activity_log_retention_days")]
    pub activity_log_retention_days: u32,
    #[serde(default = "default_jellyfin_version")]
    pub jellyfin_version: String,
}

fn default_jellyfin_version() -> String {
    "10.11.8".to_string()
}

fn default_remuxdb_url() -> Option<String> {
    Some("https://remuxdb.1632022.xyz".to_string())
}

fn default_activity_log_retention_days() -> u32 {
    90
}

fn default_tmdb_base_url() -> String {
    "https://api.themoviedb.org/3/".to_string()
}

fn default_trakt_base_url() -> String {
    "https://api.trakt.tv".to_string()
}

fn default_bgutil_script_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/usr/local/bin/bgutil-pot")
}

fn default_slow_query_threshold_ms() -> u64 {
    10_000
}

fn ensure_writable_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let _probe = tempfile::NamedTempFile::new_in(path)?;
    Ok(())
}

fn select_transcode_dir(
    primary: std::path::PathBuf,
    fallback: std::path::PathBuf,
) -> std::path::PathBuf {
    let primary_error = match ensure_writable_directory(&primary) {
        Ok(()) => return primary,
        Err(error) => error,
    };

    match ensure_writable_directory(&fallback) {
        Ok(()) => {
            warn!(
                primary = %primary.display(),
                fallback = %fallback.display(),
                error = %primary_error,
                "transcode directory is not writable; using temporary storage"
            );
            fallback
        }
        Err(fallback_error) => {
            warn!(
                primary = %primary.display(),
                fallback = %fallback.display(),
                primary_error = %primary_error,
                fallback_error = %fallback_error,
                "transcode and fallback directories are not writable"
            );
            primary
        }
    }
}

fn resolve_transcode_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    select_transcode_dir(
        data_dir.join("transcode_sessions"),
        std::env::temp_dir().join("remux-transcode"),
    )
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
        if self
            .database_url
            .is_none()
        {
            self.database_url = Some(format!(
                "sqlite://{}?mode=rwc",
                self.data_dir
                    .join("db.sqlite")
                    .display()
            ));
        }
        if self
            .torrent_data_dir
            .is_none()
        {
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
            slow_query_threshold_ms: default_slow_query_threshold_ms(),
            disable_dht: false,
            torrent_peer_port: default_torrent_peer_port(),
            bgutil_script_path: default_bgutil_script_path(),
            tmdb_base_url: default_tmdb_base_url(),
            trakt_base_url: default_trakt_base_url(),
            remuxdb_url: Some("https://remuxdb.1632022.xyz".to_string()),
            activity_log_retention_days: default_activity_log_retention_days(),
            jellyfin_version: default_jellyfin_version(),
        }
        .resolve()
    }
}

pub fn rewrite_request_uri<B>(mut req: http::Request<B>) -> http::Request<B> {
    let uri = req.uri();
    let path = uri
        .path()
        .replace("/emby", "");

    // Trim trailing slashes, but keep "/" for root, and preserve the trailing slash
    // on SPA mount prefixes (/web/, /admin/, /jellyfin/) so WebClientService and
    // ServeDir can properly resolve relative asset URLs without infinite redirect loops.
    let trimmed = path.trim_end_matches('/');
    let path = if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.eq_ignore_ascii_case("/web")
        || trimmed.eq_ignore_ascii_case("/admin")
        || trimmed.eq_ignore_ascii_case("/jellyfin")
    {
        if path.ends_with('/') {
            format!("{trimmed}/")
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };

    // Keep file paths case-sensitive (Linux filesystems are case-sensitive).
    // Only normalize API-style routes that don't look like files, plus known
    // API file endpoints (for example /Videos/.../Stream.vtt).
    let last_segment = path
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let is_file_like = last_segment.contains('.');
    let lower_path = path.to_ascii_lowercase();
    let api_file_like = is_file_like
        && (lower_path.starts_with("/videos/")
            || lower_path.starts_with("/audio/")
            || lower_path.starts_with("/items/")
            || lower_path.starts_with("/mediasegments/")
            || lower_path.starts_with("/sessions/"));

    let smart_lower_path: String = path
        .split('/')
        .map(|seg| {
            if seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
            {
                seg.to_ascii_lowercase()
            } else {
                seg.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    let new_path = if path == "/" {
        path
    } else if is_file_like && !api_file_like {
        // Static file — preserve original casing.
        path
    } else if api_file_like {
        // Known API file endpoint (HLS, VTT …) — full lowercase so the route
        // and file-extension both match our lowercase handler definitions.
        lower_path
    } else {
        // Regular API route — lowercase route keywords, preserve param values.
        smart_lower_path
    };

    let query = uri
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    let new_uri = http::Uri::builder()
        .path_and_query(format!("{}{}", new_path, query))
        .build()
        .unwrap_or_else(|_| uri.clone());

    *req.uri_mut() = new_uri;
    req
}

pub fn setup_logging(log_dir: Option<&std::path::Path>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,remux=info"));

    let fmt_layer = fmt::layer()
        .with_timer(fmt::time::ChronoLocal::new("%H:%M:%S".to_string()))
        .with_target(true)
        .with_line_number(true)
        .with_file(false)
        .compact();

    let file_layer = log_dir.map(|dir| {
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("remux")
            .filename_suffix("log")
            .build(dir)
            .expect("failed to create log appender");
        fmt::layer()
            .with_ansi(false)
            .with_timer(fmt::time::ChronoLocal::new("%H:%M:%S".to_string()))
            .with_target(true)
            .with_line_number(true)
            .with_file(false)
            .compact()
            .with_writer(appender)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(file_layer)
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
            error!(
                status = %status,
                title = %err.title(),
                detail = %err.detail(),
                cause = %format!("{:#}", cause),
                "api error"
            );
        } else {
            debug!(
                status = %status,
                title = %err.title(),
                detail = %err.detail(),
                cause = %format!("{:#}", cause),
                "api error"
            );
        }
    } else if is_server_error {
        error!(
            status = %status,
            title = %err.title(),
            detail = %err.detail(),
            "api error"
        );
    } else {
        debug!(
            status = %status,
            title = %err.title(),
            detail = %err.detail(),
            "api error"
        );
    }
}

async fn handle_static_404(req: Request<Body>) -> ApiResult<impl IntoResponse> {
    debug!(
        "Static 404 Not Found: {} {}",
        req.method(),
        req.uri()
            .path()
    );
    Ok((StatusCode::NOT_FOUND, "404 - File not found"))
}

#[cfg(test)]
pub mod integration_test;

#[cfg(test)]
mod rewrite_uri_tests {
    use super::rewrite_request_uri;

    fn rewrite(path: &str) -> String {
        let req = http::Request::builder()
            .method("GET")
            .uri(path)
            .body(())
            .unwrap();
        rewrite_request_uri(req)
            .uri()
            .path()
            .to_string()
    }

    #[test]
    fn lowercases_alpha_keyword_segments() {
        assert_eq!(rewrite("/Items/Sessions"), "/items/sessions");
        assert_eq!(rewrite("/Genres"), "/genres");
    }

    #[test]
    fn lowercases_keyword_with_digits() {
        assert_eq!(rewrite("/Items/Filters2"), "/items/filters2");
        assert_eq!(rewrite("/Hls1"), "/hls1");
    }

    #[test]
    fn preserves_uuid_param_values() {
        assert_eq!(
            rewrite("/Items/f27caa37-e514-2225-cced-ed48f6553502"),
            "/items/f27caa37-e514-2225-cced-ed48f6553502"
        );
        assert_eq!(
            rewrite("/Items/F27CAA37-E514-2225-CCED-ED48F6553502"),
            "/items/F27CAA37-E514-2225-CCED-ED48F6553502"
        );
    }

    #[test]
    fn leaves_special_char_device_ids_alone() {
        let path = "/Sessions/Play/YWJjMTIz%7Cabc";
        let rewritten = rewrite(path);
        assert!(rewritten.starts_with("/sessions/play/"));
        assert!(rewritten.contains("YWJjMTIz%7Cabc"));
    }

    #[test]
    fn strips_trailing_slashes_from_api_routes() {
        assert_eq!(rewrite("/Items/"), "/items");
        assert_eq!(rewrite("/items/"), "/items");
        assert_eq!(rewrite("/Search/Hints/"), "/search/hints");
        assert_eq!(rewrite("/Genres/"), "/genres");
        assert_eq!(rewrite("/emby/Items/"), "/items");
        assert_eq!(
            rewrite("/Items/f27caa37-e514-2225-cced-ed48f6553502/"),
            "/items/f27caa37-e514-2225-cced-ed48f6553502"
        );
        assert_eq!(
            rewrite("/Users/f27caa37-e514-2225-cced-ed48f6553502/Items/"),
            "/users/f27caa37-e514-2225-cced-ed48f6553502/items"
        );
    }

    #[test]
    fn strips_multiple_trailing_slashes() {
        assert_eq!(rewrite("/Items///"), "/items");
        assert_eq!(rewrite("/Genres//"), "/genres");
    }

    #[test]
    fn preserves_root_and_emby_root() {
        assert_eq!(rewrite("/"), "/");
        assert_eq!(rewrite("///"), "/");
        assert_eq!(rewrite("/emby"), "/");
        assert_eq!(rewrite("/emby/"), "/");
    }

    #[test]
    fn preserves_spa_mount_trailing_slashes() {
        assert_eq!(rewrite("/web"), "/web");
        assert_eq!(rewrite("/web/"), "/web/");
        assert_eq!(rewrite("/admin"), "/admin");
        assert_eq!(rewrite("/admin/"), "/admin/");
        assert_eq!(rewrite("/jellyfin"), "/jellyfin");
        assert_eq!(rewrite("/jellyfin/"), "/jellyfin/");
    }

    #[test]
    fn preserves_query_string_with_trailing_slash() {
        let req = http::Request::builder()
            .method("GET")
            .uri("/Items/?Limit=50&ParentId=123")
            .body(())
            .unwrap();
        let rewritten = rewrite_request_uri(req);
        assert_eq!(
            rewritten
                .uri()
                .path(),
            "/items"
        );
        assert_eq!(
            rewritten
                .uri()
                .query(),
            Some("Limit=50&ParentId=123")
        );
    }

    #[test]
    fn normalizes_file_endpoints_with_trailing_slash() {
        assert_eq!(rewrite("/Videos/123/Stream.vtt/"), "/videos/123/stream.vtt");
    }
}

#[cfg(test)]
mod transcode_dir_tests {
    use super::select_transcode_dir;

    #[test]
    fn uses_writable_primary_directory() {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp
            .path()
            .join("primary");
        let fallback = temp
            .path()
            .join("fallback");

        assert_eq!(select_transcode_dir(primary.clone(), fallback), primary);
    }

    #[test]
    fn falls_back_when_primary_directory_cannot_be_created() {
        let temp = tempfile::tempdir().unwrap();
        let blocking_file = temp
            .path()
            .join("not-a-directory");
        std::fs::write(&blocking_file, b"blocked").unwrap();
        let primary = blocking_file.join("transcode_sessions");
        let fallback = temp
            .path()
            .join("fallback");

        assert_eq!(select_transcode_dir(primary, fallback.clone()), fallback);
    }
}

#[cfg(all(test, unix))]
mod open_file_limit_tests {
    use super::{TARGET_OPEN_FILE_LIMIT, desired_open_file_limit};

    #[test]
    fn raises_soft_limit_to_target_without_changing_hard_limit() {
        assert_eq!(
            desired_open_file_limit(256, libc::RLIM_INFINITY),
            Some(TARGET_OPEN_FILE_LIMIT)
        );
        assert_eq!(
            desired_open_file_limit(TARGET_OPEN_FILE_LIMIT, libc::RLIM_INFINITY),
            None
        );
    }

    #[test]
    fn clamps_soft_limit_to_the_existing_hard_limit() {
        assert_eq!(desired_open_file_limit(256, 4096), Some(4096));
        assert_eq!(desired_open_file_limit(4096, 4096), None);
    }
}
