use async_trait::async_trait;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::response::Response;
use axum_anyhow::{ApiResult as Result, ResultExt};
use futures_util::TryStreamExt;
use std::io;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;

// ── StreamDescriptor (data) ───────────────────────────────────────────────────

/// Typed representation of a stream descriptor stored as JSON in `db::Media.url`.
///
/// Each variant maps to a [`StreamSource`] implementation via [`into_source`],
/// or for addon-owned streams, to the addon's [`AddonKind::serve_stream`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum StreamDescriptor {
    Http(String),
    Local(PathBuf),
    Torrent {
        info_hash: String,
        /// Filename hint for multi-file torrents (matched by name).
        file_hint: Option<String>,
        /// Direct file index within the torrent (takes precedence over file_hint).
        file_idx: Option<usize>,
        /// Tracker announce URLs (populated from the stream's `sources`).
        #[serde(default)]
        trackers: Vec<String>,
    },
    Opendal {
        addon_id: Uuid,
        path: String,
    },
}

impl StreamDescriptor {
    /// Input URL/path for ffprobe and ffmpeg (server-side tools).
    /// `Local` → raw filesystem path. `Http` → URL as-is.
    /// `Torrent`/`Opendal` → our stream proxy, which resolves them on demand.
    pub fn server_input(&self, media_id: Uuid, port: u16) -> String {
        match self {
            Self::Http(url) => url.clone(),
            Self::Local(path) => path.to_string_lossy().into_owned(),
            Self::Torrent { .. } | Self::Opendal { .. } => {
                format!("http://127.0.0.1:{}/stream/{}", port, media_id)
            }
        }
    }

    /// URL to hand to the Jellyfin client for direct play.
    /// `Http` streams play directly. Everything else routes through our stream proxy
    /// (client can't access local FS; Torrent/Opendal need server-side resolution).
    pub fn client_url(&self, media_id: Uuid, server_base: &str) -> String {
        match self {
            Self::Http(url) => url.clone(),
            _ => format!("{}/stream/{}", server_base.trim_end_matches('/'), media_id),
        }
    }

    /// The raw HTTP URL for `Http` variants, or `None` for everything else.
    pub fn as_http_url(&self) -> Option<&str> {
        match self {
            Self::Http(url) => Some(url),
            _ => None,
        }
    }

    /// If this descriptor is owned by an addon (needs its credentials/config to
    /// serve), return the addon's ID so the endpoint can dispatch to
    /// `AddonKind::serve_stream` instead of `into_source`.
    pub fn addon_id(&self) -> Option<Uuid> {
        match self {
            Self::Opendal { addon_id, .. } => Some(*addon_id),
            _ => None,
        }
    }

    /// Instantiate the runtime service for self-contained variants.
    /// Do **not** call this for `Opendal` — those must go through the addon.
    pub fn into_source(self) -> Box<dyn StreamSource> {
        match self {
            Self::Http(url) => Box::new(HttpSource { url }),
            Self::Local(path) => Box::new(LocalSource { path }),
            Self::Torrent {
                info_hash,
                file_hint,
                file_idx,
                trackers,
            } => Box::new(TorrentSource {
                info_hash,
                file_hint,
                file_idx,
                trackers,
            }),
            Self::Opendal { .. } => {
                panic!("Opendal descriptors must be served through their addon")
            }
        }
    }
}

// ── sqlx ──────────────────────────────────────────────────────────────────────

impl sqlx::Type<sqlx::Sqlite> for StreamDescriptor {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as sqlx::Type<sqlx::Sqlite>>::type_info()
    }
    fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for StreamDescriptor {
    fn decode(
        value: sqlx::sqlite::SqliteValueRef<'r>,
    ) -> std::result::Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
        if s.is_empty() {
            return Err("empty url".into());
        }
        // JSON path (new format).
        if s.starts_with('{') {
            return Ok(serde_json::from_str(&s)?);
        }
        // Legacy URL string — convert on the fly until the migration catches up.
        if s.starts_with("http://") || s.starts_with("https://") {
            return Ok(Self::Http(s));
        }
        if let Some(path) = s.strip_prefix("file://") {
            return Ok(Self::Local(std::path::PathBuf::from(path)));
        }
        Err(format!("unrecognised stream url: {s}").into())
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for StreamDescriptor {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> std::result::Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let json = serde_json::to_string(self)?;
        <String as sqlx::Encode<sqlx::Sqlite>>::encode_by_ref(&json, buf)
    }
}

// ── StreamSource (service trait) ──────────────────────────────────────────────

/// A runtime service that can serve stream bytes as an HTTP response.
///
/// Implemented by self-contained variants (`Http`, `Local`, `Torrent`).
/// Addon-owned variants (`Opendal`) are served through `AddonKind::serve_stream`.
#[async_trait]
pub trait StreamSource: Send + Sync {
    async fn serve(&self, state: &AppState, headers: &HeaderMap) -> Result<Response>;
}

// ── Concrete implementations ──────────────────────────────────────────────────

pub struct HttpSource {
    pub url: String,
}

pub struct LocalSource {
    pub path: PathBuf,
}

/// Public trackers used as fallback when a torrent stream provides none.
/// Sourced from https://github.com/ngosang/trackerslist (trackers_best).
const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://wepzone.net:6969/announce",
    "udp://tracker.srv00.com:6969/announce",
];

pub struct TorrentSource {
    pub info_hash: String,
    pub file_hint: Option<String>,
    pub file_idx: Option<usize>,
    pub trackers: Vec<String>,
}

impl TorrentSource {
    fn to_magnet(&self) -> String {
        let mut m = format!("magnet:?xt=urn:btih:{}", self.info_hash);
        let trackers: &[String] = &self.trackers;
        if trackers.is_empty() {
            for t in DEFAULT_TRACKERS {
                m.push_str(&format!("&tr={}", urlencoding::encode(t)));
            }
        } else {
            for t in trackers {
                m.push_str(&format!("&tr={}", urlencoding::encode(t)));
            }
        }
        if let Some(idx) = self.file_idx {
            m.push_str(&format!("&file_idx={}", idx));
        }
        if let Some(hint) = &self.file_hint {
            m.push_str(&format!("&file={}", urlencoding::encode(hint)));
        }
        m
    }
}

#[async_trait]
impl StreamSource for HttpSource {
    async fn serve(&self, _state: &AppState, headers: &HeaderMap) -> Result<Response> {
        let mut req = reqwest::Client::new().get(&self.url);
        if let Some(v) = headers.get(http::header::RANGE) {
            req = req.header(http::header::RANGE, v.clone());
        }

        let upstream = req
            .send()
            .await
            .context_bad_request("stream", "upstream request failed")?;

        let status = upstream.status();
        let upstream_headers = upstream.headers().clone();
        let body = Body::from_stream(upstream.bytes_stream().map_err(io::Error::other));

        let mut resp = Response::builder().status(status).body(body).unwrap();
        let out = resp.headers_mut();
        for (k, v) in &upstream_headers {
            match k.as_str() {
                "content-length" | "content-type" | "accept-ranges"
                | "content-range" | "last-modified" => {
                    out.insert(k, v.clone());
                }
                _ => {}
            }
        }
        if !out.contains_key(http::header::CONTENT_TYPE) {
            out.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/octet-stream"),
            );
        }

        Ok(resp)
    }
}

#[async_trait]
impl StreamSource for LocalSource {
    async fn serve(&self, _state: &AppState, headers: &HeaderMap) -> Result<Response> {
        let file = tokio::fs::File::open(&self.path)
            .await
            .context_not_found("stream", "file not found")?;
        let metadata = file
            .metadata()
            .await
            .context_bad_request("stream", "failed to read file metadata")?;
        let file_size = metadata.len();
        let content_type = mime_from_path(&self.path);

        let range_str = headers
            .get(http::header::RANGE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        if let Some(range) = range_str {
            let (start, end) = parse_range(&range, file_size)
                .context_bad_request("stream", "invalid Range header")?;
            let length = end - start + 1;

            let mut file = file;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .context_bad_request("stream", "seek failed")?;

            let body = Body::from_stream(ReaderStream::new(file.take(length)));

            Ok(Response::builder()
                .status(http::StatusCode::PARTIAL_CONTENT)
                .header(http::header::CONTENT_TYPE, content_type)
                .header(http::header::CONTENT_LENGTH, length)
                .header(http::header::ACCEPT_RANGES, "bytes")
                .header(
                    http::header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .body(body)
                .unwrap())
        } else {
            let body = Body::from_stream(ReaderStream::new(file));

            Ok(Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, content_type)
                .header(http::header::CONTENT_LENGTH, file_size)
                .header(http::header::ACCEPT_RANGES, "bytes")
                .body(body)
                .unwrap())
        }
    }
}

#[async_trait]
impl StreamSource for TorrentSource {
    async fn serve(&self, state: &AppState, headers: &HeaderMap) -> Result<Response> {
        let cfg = crate::db::Settings::get_config(&state.ctx.db).await?;
        if !cfg.p2p_enabled.unwrap_or(true) {
            return Err(anyhow::anyhow!("P2P disabled")).context_bad_request(
                "stream",
                "P2P streams are disabled by the server administrator",
            );
        }
        let resolved = state
            .ctx
            .torrent
            .resolve_url(&self.to_magnet())
            .await
            .context_bad_request("stream", "failed to resolve torrent")?;

        HttpSource { url: resolved }.serve(state, headers).await
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn parse_range(range: &str, file_size: u64) -> anyhow::Result<(u64, u64)> {
    let bytes = range
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow::anyhow!("expected bytes= prefix"))?;
    let (start_str, end_str) = bytes
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("malformed range"))?;

    if start_str.is_empty() {
        let suffix: u64 = end_str.parse()?;
        return Ok((file_size.saturating_sub(suffix), file_size - 1));
    }

    let start: u64 = start_str.parse()?;
    let end: u64 = if end_str.is_empty() {
        file_size - 1
    } else {
        end_str.parse::<u64>()?.min(file_size - 1)
    };

    Ok((start, end))
}

pub fn mime_from_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("ts") => "video/mp2t",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("aac") => "audio/aac",
        Some("ogg") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("m4a") => "audio/mp4",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Extract the `urn:btih:` info-hash from a magnet URI.
fn extract_btih(magnet: &str) -> Option<String> {
    url::Url::parse(magnet)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "xt")
        .and_then(|(_, v)| v.strip_prefix("urn:btih:").map(|h| h.to_ascii_lowercase()))
}

fn extract_query_param(url: &str, param: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == param)
        .map(|(_, v)| v.into_owned())
}
