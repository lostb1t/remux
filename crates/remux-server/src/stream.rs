use crate::ResultExt;
use async_trait::async_trait;
use axum::{body::Body, http::HeaderMap, response::Response};
use axum_anyhow::ApiResult as Result;
use futures_util::{StreamExt, TryStreamExt};
use nutype::nutype;
use std::{io, path::PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::{ReaderStream, StreamReader};
use uuid::Uuid;

use crate::AppState;

/// A BitTorrent tracker announce URL with a supported scheme and host. UDP
/// trackers may omit a path, while HTTP trackers must name an announce path.
/// Unrelated addon `sources` entries are rejected.
#[nutype(
    validate(predicate = is_tracker_url),
    derive(Clone, Debug, PartialEq, Eq, Hash, AsRef, Serialize, Deserialize)
)]
pub struct TrackerUrl(String);

/// Whether `s` is a usable HTTP(S) or UDP tracker URL.
pub fn is_tracker_url(s: &str) -> bool {
    let Ok(url) = url::Url::parse(s.trim()) else {
        return false;
    };
    if url
        .host_str()
        .is_none()
    {
        return false;
    }
    match url.scheme() {
        "udp" => url
            .port()
            .is_some(),
        "http" | "https" => {
            !url.path()
                .is_empty()
                && url.path() != "/"
        }
        _ => false,
    }
}

/// Whether `host` names an address only reachable from the network remux
/// itself runs on: a loopback/private/link-local/CGNAT IP, a bare
/// single-label hostname (Docker Compose/Kubernetes service names), or a
/// well-known internal-only suffix (`.local`, `.internal`, `.lan`, …).
/// Used to catch stream URLs an addon resolved from inside its own container
/// that a client elsewhere on the network could never reach.
pub fn is_internal_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_internal_ip(ip);
    }
    let host = host.trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if !host.contains('.') {
        return true;
    }
    matches!(
        host.rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "local"
                | "internal"
                | "lan"
                | "home"
                | "docker"
                | "test"
                | "invalid"
                | "localdomain"
                | "localhost"
        )
    )
}

fn is_internal_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || is_cgnat_v4(v4)
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || is_unique_local_v6(v6)
                || is_link_local_v6(v6)
        }
    }
}

/// 100.64.0.0/10, the carrier-grade NAT range Docker/Tailscale sometimes use.
fn is_cgnat_v4(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000
}

fn is_unique_local_v6(v6: std::net::Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

fn is_link_local_v6(v6: std::net::Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod internal_host_tests {
    use super::is_internal_host;

    #[test]
    fn flags_private_and_loopback_ips() {
        assert!(is_internal_host("127.0.0.1"));
        assert!(is_internal_host("192.168.1.50"));
        assert!(is_internal_host("10.0.10.66"));
        assert!(is_internal_host("172.17.0.2"));
        assert!(is_internal_host("169.254.1.1"));
        assert!(is_internal_host("100.64.0.5"));
        assert!(is_internal_host("::1"));
        assert!(is_internal_host("fc00::1"));
        assert!(is_internal_host("fe80::1"));
    }

    #[test]
    fn flags_bare_and_internal_suffixed_hostnames() {
        assert!(is_internal_host("aiostreams"));
        assert!(is_internal_host("localhost"));
        assert!(is_internal_host("myaddon.local"));
        assert!(is_internal_host("service.internal"));
        assert!(is_internal_host("box.lan"));
        assert!(is_internal_host("service.localhost"));
    }

    #[test]
    fn allows_public_ips_and_domains() {
        assert!(!is_internal_host("8.8.8.8"));
        assert!(!is_internal_host("example.com"));
        assert!(!is_internal_host("stream.example.co.uk"));
        assert!(!is_internal_host("aiostreams.example.com"));
    }
}

fn deserialize_tracker_urls<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<TrackerUrl>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = <Vec<String> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| TrackerUrl::try_new(value).ok())
        .collect())
}

/// Typed representation of how a stream is accessed (transport mechanism).
///
/// Each variant maps to a [`StreamSource`] implementation via [`into_source`],
/// or for addon-owned streams, to the addon's [`AddonKind::serve_stream`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum StreamDescriptor {
    Http {
        url: String,
        /// HTTP request headers to send when fetching this stream.
        #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
        request_headers: std::collections::HashMap<String, String>,
        /// HTTP response headers to forward to the client.
        #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
        response_headers: std::collections::HashMap<String, String>,
    },
    Local(PathBuf),
    Rtsp {
        url: String,
    },
    Torrent {
        info_hash: String,
        /// Filename hint for multi-file torrents (matched by name).
        file_hint: Option<String>,
        /// Direct file index within the torrent (takes precedence over file_hint).
        file_idx: Option<usize>,
        /// Tracker announce URLs (populated from the stream's `sources`).
        #[serde(default, deserialize_with = "deserialize_tracker_urls")]
        trackers: Vec<TrackerUrl>,
    },
    Opendal {
        addon_id: Uuid,
        path: String,
    },
}

impl Default for StreamDescriptor {
    fn default() -> Self {
        Self::Http {
            url: String::new(),
            request_headers: Default::default(),
            response_headers: Default::default(),
        }
    }
}

impl StreamDescriptor {
    /// True only for a genuine on-disk file — reading it costs nothing beyond
    /// local disk I/O. Every other variant (`Http`/`Rtsp`/`Torrent`/`Opendal`)
    /// means network I/O of some form, even when proxied through our own
    /// stream endpoint. Used to gate operations that are cheap for local files
    /// but expensive (a full remote read) for anything else — e.g. on-demand
    /// subtitle extraction.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    pub fn http(url: impl Into<String>) -> Self {
        Self::Http {
            url: url.into(),
            request_headers: Default::default(),
            response_headers: Default::default(),
        }
    }

    pub fn rtsp(url: impl Into<String>) -> Self {
        Self::Rtsp { url: url.into() }
    }

    /// Input URL/path for ffprobe and ffmpeg (server-side tools).
    /// `Local` → raw filesystem path. `Http` → URL as-is.
    /// `Torrent`/`Opendal` → our stream proxy, which resolves them on demand.
    pub fn server_input(&self, media_id: Uuid, port: u16) -> String {
        match self {
            Self::Http { url, .. } | Self::Rtsp { url } => url.clone(),
            Self::Local(path) => path
                .to_string_lossy()
                .into_owned(),
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
            Self::Http { url, .. } => url.clone(),
            _ => format!("{}/stream/{}", server_base.trim_end_matches('/'), media_id),
        }
    }

    /// The raw HTTP URL for `Http` variants, or `None` for everything else.
    pub fn as_http_url(&self) -> Option<&str> {
        match self {
            Self::Http { url, .. } => Some(url),
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

    /// Returns `false` for HTTP streams whose ranged GET gets no response,
    /// returns 4xx/5xx, lands on an AIOStreams error clip, or starts with an
    /// archive signature; body-read errors leave the stream alive. Non-HTTP
    /// variants return `true`.
    pub async fn is_alive(&self) -> bool {
        let Self::Http {
            url,
            request_headers,
            ..
        } = self
        else {
            return true;
        };
        let mut request = AVAILABILITY_CLIENT.get(url);
        for (key, value) in request_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        request = request.header(http::header::RANGE, "bytes=0-7");

        let Ok(mut response) = request
            .send()
            .await
        else {
            return false;
        };
        if response
            .status()
            .as_u16()
            >= 400
            || response
                .url()
                .path()
                .rsplit_once("/static/")
                .is_some_and(|(_, clip)| AIOSTREAMS_ERROR_CLIPS.contains(&clip))
        {
            return false;
        }

        // Read only until the magic-number prefix is available; servers that
        // ignore Range must not trigger a full download.
        let mut head = Vec::with_capacity(8);
        while head.len() < 8 {
            match response
                .chunk()
                .await
            {
                Ok(Some(chunk)) => head.extend_from_slice(&chunk),
                _ => break,
            }
        }
        !ARCHIVE_MAGIC
            .iter()
            .any(|magic| head.starts_with(magic))
    }

    /// Instantiate the runtime service for self-contained variants.
    /// Do **not** call this for `Opendal` — those must go through the addon.
    pub fn into_source(self) -> Box<dyn StreamSource> {
        match self {
            Self::Http {
                url,
                request_headers,
                response_headers,
            } => Box::new(HttpSource {
                url,
                request_headers,
                response_headers,
            }),
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
            Self::Rtsp { .. } => {
                panic!("Rtsp descriptors must be served through the transcode path")
            }
            Self::Opendal { .. } => {
                panic!("Opendal descriptors must be served through their addon")
            }
        }
    }
}

/// Combined stream descriptor and provider metadata stored in `db::Media.stream_info`.
///
/// Replaces the old split between `db::Media.url` (transport) and
/// `db::Media.provider_info` (Stremio metadata). All addons populate whichever
/// fields they have; the rest are `None` / empty.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StreamInfo {
    /// `#[serde(default)]` so deserializing a redacted `remux.provider_info`
    /// (see `to_public_json`, which strips this key before it reaches the
    /// API) doesn't fail — `device_profile.rs`'s `source_stream_info` round-trips
    /// that JSON back into a `StreamInfo` just to read `filename`.
    #[serde(default)]
    pub descriptor: StreamDescriptor,
    /// Filename from the provider (e.g. "Movie.2021.1080p.BluRay.mkv").
    /// Used for resolution matching during probe fallback.
    pub filename: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Addon that produced this stream (stamped by the service layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// UUID of the addon that produced this stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addon_id: Option<Uuid>,
    /// Lowercased service identifier from `streamData.service.id` (e.g. "real-debrid").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    /// Whether the upstream service confirmed this stream is cached. Missing
    /// metadata is not a cache hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_cached: Option<bool>,
    pub seeders: Option<i64>,
    pub size: Option<i64>,
    pub duration: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<crate::sdks::stremio::Subtitle>,
    /// Catchup URL template from M3U `catchup-source` attribute.
    /// `{utc}` / `{utcend}` placeholders are substituted at playback time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catchup_source: Option<String>,
    /// Number of days of catchup available (`catchup-days` attribute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catchup_days: Option<i64>,
    /// Usenet NZB GUID (the `id` query param from the nzb_url). Used for RemuxDB matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usenet_guid: Option<String>,
    /// Usenet indexer name (e.g. "NZBgeek"). Used for RemuxDB matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usenet_indexer: Option<String>,
    /// Raw NZB URL (from AIOStreams streamData). Used for RemuxDB matching via indexer_guid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nzb_url: Option<String>,
    /// Binge group from `behaviorHints.bingeGroup` (e.g. "real-debrid|1080p").
    /// Used as part of the stable dedup key for HTTP streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binge_group: Option<String>,
    /// Upstream addon name from `streamData.addon` (e.g. "BB", "Comet").
    /// Used alongside `service_id` for the addon-level stream filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_addon: Option<String>,
    /// Torrent info-hash for the source release (from AIOStreams streamData).
    /// Stored independently of the descriptor so debrid Http streams can match and submit by hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_info_hash: Option<String>,
    /// File index within the torrent (from AIOStreams streamData).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_file_idx: Option<i32>,
    /// Pre-probed codec/bitrate metadata from the addon.
    /// Extracted into `db::Media.probe_data` on conversion; not persisted here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_data: Option<crate::api::MediaSourceInfo>,
}

/// Sizes shown to three significant digits ("11.9 GB") are off by up to 1/200.
const LISTED_SIZE_PRECISION_DIVISOR: u64 = 200;
/// ±0.005 GiB: sizes shown in GiB to two decimals ("0.68 GB") are off by up to
/// half a step, which exceeds 1/200 below 1 GiB.
const LISTED_SIZE_ROUNDING_FLOOR: u64 = (1 << 30) / 200;

impl StreamInfo {
    pub fn is_p2p(&self) -> bool {
        matches!(self.descriptor, StreamDescriptor::Torrent { .. })
    }

    /// Redacted view for `remux.provider_info` in API responses. `descriptor`
    /// carries provider credentials (debrid keys in the URL, IPTV
    /// username/password, upstream request headers) that must never reach a
    /// non-admin client — no client reads it, the only consumer is
    /// `device_profile.rs` reading `filename` back out, which stays. Internal
    /// persistence (`db::Media.stream_info`) uses `StreamInfo`'s own
    /// `Serialize` impl directly and keeps `descriptor` intact.
    pub fn to_public_json(&self) -> Option<serde_json::Value> {
        let mut value = serde_json::to_value(self).ok()?;
        if let Some(obj) = value.as_object_mut() {
            obj.remove("descriptor");
        }
        Some(value)
    }

    /// Torrent identity used for RemuxDB lookup and submission.
    ///
    /// A `Torrent` descriptor is authoritative. Any other transport (notably a
    /// debrid `Http` stream) falls back to the identity preserved from
    /// AIOStreams `streamData.torrent`. `None` when the stream has no torrent
    /// identity at all.
    pub fn torrent_identity(&self) -> Option<(&str, Option<i32>)> {
        match &self.descriptor {
            StreamDescriptor::Torrent {
                info_hash,
                file_idx,
                ..
            } => Some((
                info_hash.as_str(),
                // Indices beyond i32 can't be represented in RemuxDB; treat as unknown.
                file_idx.and_then(|i| i32::try_from(i).ok()),
            )),
            _ => self
                .torrent_info_hash
                .as_deref()
                .map(|hash| (hash, self.torrent_file_idx)),
        }
    }

    /// Whether a probed file of `probed_size` bytes can't be the listed one,
    /// i.e. a resolver failover served another release. remux's own torrent
    /// engine never fails over to another release.
    pub fn is_other_file(&self, probed_size: Option<i64>) -> bool {
        let (Some(listed), Some(probed)) = (
            self.size
                .filter(|&s| s > 0),
            probed_size.filter(|&s| s > 0),
        ) else {
            return false;
        };
        let tolerance = (listed.max(probed) as u64 / LISTED_SIZE_PRECISION_DIVISOR)
            .max(LISTED_SIZE_ROUNDING_FLOOR);
        !self.is_p2p()
            && listed.abs_diff(probed) > tolerance
            && (probed > listed || !self.may_list_release())
    }

    /// NZBs and torrents without a file index may list the whole release,
    /// which no file in it can exceed.
    fn may_list_release(&self) -> bool {
        self.usenet_guid
            .is_some()
            || self
                .nzb_url
                .is_some()
            || matches!(self.torrent_identity(), Some((_, None)))
    }

    pub fn resolution_tag(&self) -> Option<String> {
        let src = self
            .filename
            .as_deref()
            .or(self
                .name
                .as_deref())?;
        crate::db::min_screen_size(&hunch::hunch(src)).map(|s| s.to_owned())
    }
}

/// A runtime service that can serve stream bytes as an HTTP response.
///
/// Implemented by self-contained variants (`Http`, `Local`, `Torrent`).
/// Addon-owned variants (`Opendal`) are served through `AddonKind::serve_stream`.
#[async_trait]
pub trait StreamSource: Send + Sync {
    async fn serve(&self, state: &AppState, headers: &HeaderMap) -> Result<Response>;
}

static AVAILABILITY_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("failed to build availability client")
    });

/// Clips AIOStreams redirects to (`/static/<clip>`) when a debrid resolve
/// fails; they are playable MP4s, so status alone reads them as alive. Mirrors
/// `StaticFiles` in AIOStreams `utils/static-errors.ts`, minus `200.mp4`.
const AIOSTREAMS_ERROR_CLIPS: &[&str] = &[
    "download_failed.mp4",
    "downloading.mp4",
    "unavailable_for_legal_reasons.mp4",
    "store_limit_exceeded.mp4",
    "content_proxy_limit_reached.mp4",
    "500.mp4",
    "429.mp4",
    "403.mp4",
    "401.mp4",
    "no_matching_file.mp4",
    "payment_required.mp4",
];

/// Archive signatures (RAR, 7z, ZIP) occasionally served by debrid links in
/// place of the video file; ffmpeg cannot play them.
const ARCHIVE_MAGIC: &[&[u8]] =
    &[b"Rar!\x1a\x07", b"7z\xbc\xaf\x27\x1c", b"PK\x03\x04"];

static STREAM_PROXY_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(20)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build stream proxy client")
    });

pub struct HttpSource {
    pub url: String,
    pub request_headers: std::collections::HashMap<String, String>,
    pub response_headers: std::collections::HashMap<String, String>,
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
    pub trackers: Vec<TrackerUrl>,
}

impl TorrentSource {
    fn to_magnet(&self) -> String {
        let mut m = format!("magnet:?xt=urn:btih:{}", self.info_hash);
        let trackers = &self.trackers;
        if trackers.is_empty() {
            for t in DEFAULT_TRACKERS {
                m.push_str(&format!("&tr={}", urlencoding::encode(t)));
            }
        } else {
            for t in trackers {
                let t: &str = t.as_ref();
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

impl HttpSource {
    async fn serve_inner(
        &self,
        headers: &HeaderMap,
        bound_finite_ranges: bool,
    ) -> Result<Response> {
        let requested_range = bound_finite_ranges
            .then(|| {
                headers
                    .get(http::header::RANGE)
                    .and_then(|value| {
                        value
                            .to_str()
                            .ok()
                    })
                    .and_then(parse_open_or_finite_range)
            })
            .flatten();
        let mut req = STREAM_PROXY_CLIENT
            .clone()
            .get(&self.url);
        if let Some((start, _)) = requested_range {
            // librqbit accepts only `bytes=N-`. Normalize finite ranges and
            // cap the proxied response body to the client's requested length.
            req = req.header(http::header::RANGE, format!("bytes={start}-"));
        } else if let Some(v) = headers.get(http::header::RANGE) {
            req = req.header(http::header::RANGE, v.clone());
        }
        for (k, v) in &self.request_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let upstream = req
            .send()
            .await
            .context_bad_request("upstream request failed")?;

        let upstream_status = upstream.status();
        let upstream_headers = upstream
            .headers()
            .clone();
        let total_length = upstream_headers
            .get(http::header::CONTENT_RANGE)
            .and_then(|value| {
                value
                    .to_str()
                    .ok()
            })
            .and_then(|value| value.rsplit_once('/'))
            .and_then(|(_, total)| {
                total
                    .parse::<u64>()
                    .ok()
            });
        let bounded_range = requested_range
            .filter(|(_, end)| {
                end.is_some() && upstream_status == http::StatusCode::PARTIAL_CONTENT
            })
            .map(|(start, end)| {
                let requested_end = end.expect("filtered to finite range");
                let end = total_length
                    .map(|total| requested_end.min(total.saturating_sub(1)))
                    .unwrap_or(requested_end);
                let length = end
                    .saturating_sub(start)
                    .saturating_add(1);
                (start, end, length)
            });
        let stream = upstream
            .bytes_stream()
            .map_err(io::Error::other);
        let body = if let Some((_, _, length)) = bounded_range {
            Body::from_stream(ReaderStream::new(StreamReader::new(stream).take(length)))
        } else {
            Body::from_stream(stream)
        };

        let mut resp = Response::builder()
            .status(if bounded_range.is_some() {
                http::StatusCode::PARTIAL_CONTENT
            } else {
                upstream_status
            })
            .body(body)
            .unwrap();
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
        if let Some((start, end, length)) = bounded_range {
            out.insert(
                http::header::CONTENT_LENGTH,
                http::HeaderValue::from_str(&length.to_string())
                    .expect("range length is a valid header"),
            );
            let total = total_length
                .map(|total| total.to_string())
                .unwrap_or_else(|| "*".to_string());
            out.insert(
                http::header::CONTENT_RANGE,
                http::HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                    .expect("range is a valid header"),
            );
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

fn parse_open_or_finite_range(range: &str) -> Option<(u64, Option<u64>)> {
    let bytes = range.strip_prefix("bytes=")?;
    let (start, end) = bytes.split_once('-')?;
    if start.is_empty() || end.contains(',') {
        return None;
    }
    let start = start
        .parse()
        .ok()?;
    let end = (!end.is_empty())
        .then(|| {
            end.parse()
                .ok()
        })
        .flatten();
    end.is_none_or(|end| end >= start)
        .then_some((start, end))
}

#[async_trait]
impl StreamSource for HttpSource {
    async fn serve(&self, _state: &AppState, headers: &HeaderMap) -> Result<Response> {
        self.serve_inner(headers, false)
            .await
    }
}

#[async_trait]
impl StreamSource for LocalSource {
    async fn serve(&self, _state: &AppState, headers: &HeaderMap) -> Result<Response> {
        let file = tokio::fs::File::open(&self.path)
            .await
            .context_not_found("file not found")?;
        let metadata = file
            .metadata()
            .await
            .context_bad_request("failed to read file metadata")?;
        let file_size = metadata.len();
        let content_type = mime_from_path(&self.path);

        let range_str = headers
            .get(http::header::RANGE)
            .and_then(|v| {
                v.to_str()
                    .ok()
            })
            .map(str::to_owned);

        if let Some(range) = range_str {
            let (start, end) = parse_range(&range, file_size)
                .context_bad_request("invalid Range header")?;
            let length = end - start + 1;

            let mut file = file;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .context_bad_request("seek failed")?;

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
        let torrent = state
            .ctx
            .torrent
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("P2P is disabled"))
            .context_bad_request("P2P is disabled")?;
        let lease = torrent
            .acquire(&self.info_hash)
            .await;
        let resolved = torrent
            .resolve_url(&self.to_magnet())
            .await
            .context_bad_request("failed to resolve torrent")?;

        let response = HttpSource {
            url: resolved,
            request_headers: Default::default(),
            response_headers: Default::default(),
        }
        .serve_inner(headers, true)
        .await?;
        let (parts, body) = response.into_parts();
        let stream = async_stream::stream! {
            let _lease = lease;
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                yield chunk;
            }
        };
        Ok(Response::from_parts(parts, Body::from_stream(stream)))
    }
}

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
        end_str
            .parse::<u64>()?
            .min(file_size - 1)
    };

    Ok((start, end))
}

pub fn mime_from_path(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    // Check audio-specific extensions first so that formats like .m4a (which
    // aliases to VideoContainer::Mp4 for profile matching) still get the
    // correct audio/* mime type.
    if let Some(c) = remux_sdks::remux::AudioContainer::parse_known(ext) {
        return c.mime_type();
    }
    if let Some(c) = remux_sdks::remux::VideoContainer::parse_known(ext) {
        return c.mime_type();
    }
    "application/octet-stream"
}

/// Extract the `urn:btih:` info-hash from a magnet URI.
fn extract_btih(magnet: &str) -> Option<String> {
    url::Url::parse(magnet)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "xt")
        .and_then(|(_, v)| {
            v.strip_prefix("urn:btih:")
                .map(|h| h.to_ascii_lowercase())
        })
}

fn extract_query_param(url: &str, param: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == param)
        .map(|(_, v)| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        HttpSource, STREAM_PROXY_CLIENT, StreamDescriptor, StreamInfo, TrackerUrl,
        is_tracker_url, mime_from_path,
    };
    use std::path::Path;

    #[test]
    fn mime_from_path_video() {
        assert_eq!(mime_from_path(Path::new("movie.mkv")), "video/x-matroska");
        assert_eq!(mime_from_path(Path::new("movie.mp4")), "video/mp4");
        assert_eq!(mime_from_path(Path::new("movie.m4v")), "video/mp4");
        assert_eq!(mime_from_path(Path::new("movie.mov")), "video/quicktime");
        assert_eq!(mime_from_path(Path::new("movie.avi")), "video/x-msvideo");
        assert_eq!(mime_from_path(Path::new("movie.webm")), "video/webm");
        assert_eq!(mime_from_path(Path::new("movie.ts")), "video/mp2t");
        assert_eq!(mime_from_path(Path::new("movie.MKV")), "video/x-matroska");
    }

    #[test]
    fn mime_from_path_audio() {
        assert_eq!(mime_from_path(Path::new("track.mp3")), "audio/mpeg");
        assert_eq!(mime_from_path(Path::new("track.flac")), "audio/flac");
        assert_eq!(mime_from_path(Path::new("track.m4a")), "audio/mp4");
        assert_eq!(mime_from_path(Path::new("track.ogg")), "audio/ogg");
        assert_eq!(mime_from_path(Path::new("track.opus")), "audio/opus");
        assert_eq!(mime_from_path(Path::new("track.wav")), "audio/wav");
        assert_eq!(mime_from_path(Path::new("track.aac")), "audio/aac");
    }

    #[test]
    fn mime_from_path_fallback() {
        assert_eq!(
            mime_from_path(Path::new("file.m3u8")),
            "application/octet-stream"
        );
        assert_eq!(
            mime_from_path(Path::new("file.txt")),
            "application/octet-stream"
        );
        assert_eq!(
            mime_from_path(Path::new("noextension")),
            "application/octet-stream"
        );
    }

    #[test]
    fn tracker_url_validates_absolute_urls() {
        assert!(is_tracker_url("udp://tracker.opentrackr.org:1337/announce"));
        assert!(is_tracker_url("udp://opentor.net:6969"));
        assert!(is_tracker_url("https://private.example/announce"));
        assert!(is_tracker_url("http://tracker.example:8080/announce"));

        // Missing announce paths for HTTP, missing UDP ports, relative paths,
        // unsupported schemes, and non-URLs are rejected.
        assert!(!is_tracker_url("https://tracker.example"));
        assert!(!is_tracker_url("udp://tracker.example"));
        assert!(!is_tracker_url("/announce"));
        assert!(!is_tracker_url("tracker:udp://x/announce"));
        assert!(!is_tracker_url("not-a-tracker"));
        assert!(
            TrackerUrl::try_new(
                "udp://tracker.opentrackr.org:1337/announce".to_string()
            )
            .is_ok()
        );
        assert!(TrackerUrl::try_new("not-a-tracker".to_string()).is_err());
    }

    #[test]
    fn torrent_descriptor_ignores_invalid_persisted_trackers() {
        let descriptor: StreamDescriptor = serde_json::from_str(
            r#"{"Torrent":{"info_hash":"abc","file_hint":null,"file_idx":0,"trackers":["udp://opentor.net:6969","not-a-tracker"]}}"#,
        )
        .unwrap();

        let StreamDescriptor::Torrent { trackers, .. } = descriptor else {
            panic!("expected torrent descriptor");
        };
        assert_eq!(trackers.len(), 1);
        assert_eq!(trackers[0].as_ref(), "udp://opentor.net:6969");
    }

    #[test]
    fn stream_proxy_client_builds_without_panic() {
        let _ = &*STREAM_PROXY_CLIENT;
    }

    #[tokio::test]
    async fn is_alive_rejects_error_clips_and_archives() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/playback/failed");
            then.status(307)
                .header("Location", "/static/download_failed.mp4");
        });
        for (path, body) in [
            ("/static/download_failed.mp4", &b"\x00\x00\x00\x20ftyp"[..]),
            ("/archive.mkv", b"Rar!\x1a\x07\x01\x00"),
            ("/movie.mkv", b"\x1a\x45\xdf\xa3\x01\x00\x00\x00"),
        ] {
            server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path(path);
                then.status(206)
                    .body(body);
            });
        }

        for (path, alive) in [
            ("/playback/failed", false),
            ("/archive.mkv", false),
            ("/movie.mkv", true),
        ] {
            assert_eq!(
                StreamDescriptor::http(server.url(path))
                    .is_alive()
                    .await,
                alive,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn torrent_proxy_bounds_finite_ranges() {
        let server = httpmock::MockServer::start();
        let payload = vec![b'x'; 2048];
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/file.mkv")
                .header("Range", "bytes=0-");
            then.status(206)
                .header("Content-Range", "bytes 0-2047/2048")
                .header("Accept-Ranges", "bytes")
                .body(payload);
        });

        let source = HttpSource {
            url: format!("{}/file.mkv", server.base_url()),
            request_headers: Default::default(),
            response_headers: Default::default(),
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::RANGE,
            http::HeaderValue::from_static("bytes=0-1023"),
        );

        let response = source
            .serve_inner(&headers, true)
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[http::header::CONTENT_LENGTH], "1024");
        assert_eq!(
            response.headers()[http::header::CONTENT_RANGE],
            "bytes 0-1023/2048"
        );
        let body = axum::body::to_bytes(response.into_body(), 2048)
            .await
            .unwrap();
        assert_eq!(body.len(), 1024);
    }

    #[tokio::test]
    async fn torrent_proxy_does_not_rewrite_full_responses_as_partial() {
        let server = httpmock::MockServer::start();
        let payload = vec![b'x'; 2048];
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/file.mkv")
                .header("Range", "bytes=0-");
            then.status(200)
                .header("Accept-Ranges", "bytes")
                .body(payload);
        });

        let source = HttpSource {
            url: format!("{}/file.mkv", server.base_url()),
            request_headers: Default::default(),
            response_headers: Default::default(),
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::RANGE,
            http::HeaderValue::from_static("bytes=0-1023"),
        );

        let response = source
            .serve_inner(&headers, true)
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 2048)
            .await
            .unwrap();
        assert_eq!(body.len(), 2048);
    }

    #[tokio::test]
    async fn stream_proxy_client_forwards_range_and_returns_206() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/file.mkv")
                .header("Range", "bytes=0-1023");
            then.status(206)
                .header("Content-Range", "bytes 0-1023/1048576")
                .header("Accept-Ranges", "bytes")
                .body(b"payload".to_vec());
        });

        let resp = STREAM_PROXY_CLIENT
            .clone()
            .get(format!("{}/file.mkv", server.base_url()))
            .header("Range", "bytes=0-1023")
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(
            resp.status()
                .as_u16(),
            206
        );
        assert_eq!(
            resp.headers()
                .get("content-range")
                .unwrap(),
            "bytes 0-1023/1048576"
        );
        resp.text()
            .await
            .expect("body should drain");
    }

    #[test]
    fn is_other_file_only_when_listing_rounding_cannot_explain_the_size() {
        let indexed = StreamInfo {
            torrent_info_hash: Some("abc123".into()),
            torrent_file_idx: Some(0),
            ..Default::default()
        };
        let unindexed = StreamInfo {
            torrent_info_hash: Some("abc123".into()),
            ..Default::default()
        };
        let nzb = StreamInfo {
            nzb_url: Some("https://indexer.example/api?t=get&id=1".into()),
            ..Default::default()
        };
        let p2p = StreamInfo {
            descriptor: StreamDescriptor::Torrent {
                info_hash: "abc123".into(),
                file_hint: None,
                file_idx: None,
                trackers: vec![],
            },
            ..Default::default()
        };
        let (listed, smaller, larger) = (12_811_223_977, 8_657_935_338, 19_000_000_000);
        let cases = [
            (&indexed, listed, smaller, true),
            (&indexed, listed, larger, true),
            // 0.33% off: within three-significant-digit rounding.
            (&indexed, listed, listed - 42_000_000, false),
            // 0.53% off, but within the 0.005 GiB rounding floor.
            (&indexed, 730_144_440, 734_003_200, false),
            (&indexed, 730_144_440, 740_000_000, true),
            // May list the whole release: only a larger file contradicts it.
            (&unindexed, listed, smaller, false),
            (&unindexed, listed, larger, true),
            (&nzb, listed, smaller, false),
            (&nzb, listed, larger, true),
            (&p2p, listed, larger, false),
        ];
        for (si, listed, probed, other) in cases {
            let si = StreamInfo {
                size: Some(listed),
                ..si.clone()
            };
            assert_eq!(
                si.is_other_file(Some(probed)),
                other,
                "{listed} vs {probed}"
            );
            assert!(!si.is_other_file(None));
        }
        assert!(!indexed.is_other_file(Some(smaller)));
    }
}
