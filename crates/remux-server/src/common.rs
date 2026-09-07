use anyhow::{Context, Result, anyhow};
use async_compression::tokio::bufread::GzipDecoder;
use tracing::{info, warn};

//use futures::Stream;
//use futures::StreamExt;
//use futures_util::TryStreamExt;
use tokio_stream::{Stream, StreamExt};
// use tokio_stream::TryStreamExt;
//use tokio_stream::TryStreamExt;
use chrono::{DateTime, NaiveDate, Utc};
use csv_async::{AsyncDeserializer, AsyncReaderBuilder};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::{path::Path, pin::Pin};
//use std::task::{Context, Poll};
use tempfile;
use tokio::{
    fs::File as TokioFile,
    io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
};
use tokio_util::{
    compat::TokioAsyncReadCompatExt,
    io::{ReaderStream, StreamReader},
};
use tracing;
//use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use crate::errors::LogErr;
use std::str::FromStr;

use moka::sync::Cache;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use crate::{api, sdks};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::Uuid;

static SERVER_ID: OnceLock<String> = OnceLock::new();

pub(crate) fn set_server_id(id: String) {
    let _ = SERVER_ID.set(id);
}

pub fn server_id() -> String {
    SERVER_ID
        .get()
        .cloned()
        .unwrap_or_else(|| "remux".to_string())
}

pub fn native_to_utc(opt_date: Option<NaiveDate>) -> Option<DateTime<Utc>> {
    opt_date
        .and_then(|d| d.and_hms_opt(0, 0, 0)) // Add time
        .map(|ndt| DateTime::<Utc>::from_utc(ndt, Utc)) // Make it UTC
}

pub async fn download_to_file(url: &str) -> Result<TokioFile> {
    let resp = reqwest::get(url)
        .await?
        .error_for_status()?;
    let bytes = resp
        .bytes()
        .await?;

    let std_file = tempfile::tempfile()?; // std::fs::File
    let mut file = TokioFile::from_std(std_file); // convert to async
    file.write_all(&bytes)
        .await?;
    file.sync_all()
        .await?;
    file.seek(std::io::SeekFrom::Start(0))
        .await?;

    Ok(file)
}

pub struct FileStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T>> + Send>>,
}

impl<T> FileStream<T>
where
    T: DeserializeOwned + Send + 'static,
{
    pub async fn from_url(url: &str) -> Result<Self> {
        let tmpfile = download_to_file(url).await?;

        // detect extension (gzip-inside)
        let path = Path::new(url);
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let inner_ext = ext.trim_end_matches(".gz");

        let reader = BufReader::new(tmpfile);
        let decoder = GzipDecoder::new(reader);
        //let buffered: Box<dyn tokio::io::AsyncBufRead + Send + Unpin> =
        //    Box::new(BufReader::new(decoder));
        let buffered = BufReader::new(decoder);

        // JSON-lines
        if matches!(inner_ext, "json" | "jsonl" | "ndjson") {
            let line_stream =
                tokio_stream::wrappers::LinesStream::new(buffered.lines());
            let json_stream = line_stream
                .then(|line_result| async move {
                    match line_result {
                        Ok(line) => match serde_json::from_str::<T>(&line) {
                            Ok(obj) => Some(Ok(obj)),
                            Err(e) => {
                                warn!("Line read error: {e} — skipping line");
                                None
                            }
                        },
                        Err(e) => {
                            warn!("Line read error: {e} — skipping line");
                            None
                        }
                    }
                })
                .filter_map(|x| x);

            return Ok(Self {
                inner: Box::pin(json_stream),
            });
        }

        // CSV/TSV fallback
        // let delimiter = if inner_ext == "tsv" { b'\t' } else { b',' };
        let delimiter = b'\t';
        let csv_reader = AsyncReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .create_deserializer(buffered);
        // .create_reader(buffered);

        let csv_stream = csv_reader
            .into_deserialize::<T>() // <-- note: deserialize, not deserializer
            .then(|res| async move {
                match res {
                    Ok(row) => {
                        // info!("sucess");
                        Some(Ok(row))
                    }
                    Err(e) => {
                        warn!("CSV parse error: {e} — skipping row");
                        None
                    }
                }
            })
            .filter_map(|x| x);
        // let csv_stream = AsyncReaderBuilder::new()
        //     .delimiter(delimiter)
        //     .has_headers(true)
        //     .create_deserializer(reader)
        //     .deserialize::<T>()
        //     .then(|res| async move {
        //         match res {
        //             Ok(row) => Some(Ok(row)),
        //             Err(e) => {
        //                 warn!("CSV parse error: {e} — skipping row");
        //                 None
        //             }
        //         }
        //     })
        //     .filter_map(|x| x);

        Ok(Self {
            inner: Box::pin(csv_stream),
        })
    }
}

pub fn parse_strings_to_u64s(strings: Vec<String>) -> Vec<u64> {
    strings
        .into_iter()
        .filter_map(|s| {
            s.parse::<u64>()
                .ok()
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub enum TickUnit {
    Ticks,
    Seconds,
    Minutes,
}

impl std::str::FromStr for TickUnit {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .to_lowercase()
            .as_str()
        {
            "ticks" => Ok(TickUnit::Ticks),
            "seconds" => Ok(TickUnit::Seconds),
            "minutes" => Ok(TickUnit::Minutes),
            _ => Err(()),
        }
    }
}

pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / 10_000_000.0
}

pub fn duration_to_ticks(value: f64, unit: TickUnit) -> i64 {
    match unit {
        TickUnit::Ticks => value.round() as i64,
        TickUnit::Seconds => (value * 10_000_000.0).round() as i64,
        TickUnit::Minutes => (value * 60.0 * 10_000_000.0).round() as i64,
    }
}

pub trait ToRunTimeTicks {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64>;
}

// Numeric types
impl ToRunTimeTicks for u32 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for u64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for i32 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for i64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for f64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self, unit))
    }
}

// Strings
impl ToRunTimeTicks for String {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        self.parse::<f64>()
            .ok()
            .and_then(|v| v.to_ticks(unit))
    }
}

impl ToRunTimeTicks for &str {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        self.parse::<f64>()
            .ok()
            .and_then(|v| v.to_ticks(unit))
    }
}

const NS: Uuid = uuid::uuid!("6ba7b810-9dad-11d1-80b4-00c04fd430c8"); // DNS namespace

pub fn get_stable_uuid(v: String) -> Uuid {
    Uuid::new_v5(&NS, v.as_bytes())
}

pub fn get_uuid() -> Uuid {
    uuid::Uuid::new_v4()
}

/// Computes the stable UUID for a media item from its kind and canonical external ID.
pub fn stable_media_uuid(kind: &crate::db::MediaKind, canonical: &str) -> Uuid {
    get_stable_uuid(format!("{}:{}", kind, canonical))
}

pub async fn tmdb_client(
    db: &sqlx::SqlitePool,
    base_url: &str,
) -> Option<sdks::RestClient<sdks::BearerAuth>> {
    let cfg = crate::db::Settings::get_config_or_default(db).await;
    tmdb_client_from_config(&cfg, base_url)
}

pub fn tmdb_client_from_config(
    cfg: &crate::api::ServerConfiguration,
    base_url: &str,
) -> Option<sdks::RestClient<sdks::BearerAuth>> {
    let key = cfg
        .get_tmdb_key()
        .to_string();
    sdks::RestClient::new(base_url)
        .ok()
        .map(|c| {
            c.with_auth(sdks::BearerAuth { token: key })
                .with_retry(
                    sdks::ExponentialBackoff::builder().build_with_max_retries(3),
                )
        })
}

/// A TVDB bearer token, cached for rather less than the month TVDB grants it
/// so a login failure surfaces on a scan rather than mid-delivery. Keyed on the
/// api key so changing it in settings takes effect without a restart.
///
/// `None` when no key is configured, which is the normal state: TheTVDB issues
/// a key per project and there is no bundled one to fall back on, unlike TMDB.
pub async fn tvdb_token(ctx: &crate::AppContext) -> Option<String> {
    let cfg = crate::db::Settings::get_config_or_default(&ctx.db).await;
    let api_key = cfg
        .tvdb_api_key
        .as_deref()
        .filter(|k| !k.is_empty())?
        .to_string();
    let pin = cfg
        .tvdb_pin
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(str::to_string);

    let cache_key = format!("tvdb-token:{api_key}");
    if let Some(token) = ctx
        .store
        .get::<String>(&cache_key)
    {
        return Some((*token).clone());
    }

    let client = sdks::tvdb::client(sdks::tvdb::BASE_URL).ok()?;
    let token = match client
        .execute(sdks::tvdb::LoginEndpoint { api_key, pin })
        .await
    {
        Ok(res) => {
            res.data
                .token
        }
        Err(e) => {
            tracing::warn!(error = %e, "tvdb login failed");
            return None;
        }
    };

    ctx.store
        .save(
            cache_key,
            token.clone(),
            std::time::Duration::from_secs(60 * 60 * 24 * 25),
        );
    Some(token)
}

/// Authenticated for everything past `/login`.
pub async fn tvdb_client(
    ctx: &crate::AppContext,
) -> Option<sdks::RestClient<sdks::BearerAuth>> {
    let token = tvdb_token(ctx).await?;
    sdks::RestClient::new(sdks::tvdb::BASE_URL)
        .ok()
        .map(|c| {
            c.with_auth(sdks::BearerAuth { token })
                .with_retry(
                    sdks::ExponentialBackoff::builder().build_with_max_retries(3),
                )
        })
}

// --- Progress reporting ---

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct ProgressReporter(Arc<dyn Fn(f64) + Send + Sync>);

impl ProgressReporter {
    pub fn new(inner: Arc<AtomicU64>) -> Self {
        Self(Arc::new(move |pct: f64| {
            let rounded = (pct.clamp(0.0, 100.0) * 10.0).round() / 10.0;
            inner.store(rounded.to_bits(), Ordering::Relaxed);
        }))
    }

    pub fn set(&self, pct: f64) {
        (self.0)(pct.clamp(0.0, 100.0));
    }

    pub fn scaled(&self, start: f64, end: f64) -> ProgressReporter {
        let parent = self.clone();
        ProgressReporter(Arc::new(move |pct: f64| {
            let mapped = start + (end - start) * pct.clamp(0.0, 100.0) / 100.0;
            parent.set(mapped);
        }))
    }

    /// Report `n` items done out of `total`. Computes the percentage automatically.
    /// When `total` is 0, reports 100%.
    pub fn report(&self, n: usize, total: usize) {
        let pct = if total == 0 {
            100.0
        } else {
            n as f64 / total as f64 * 100.0
        };
        self.set(pct);
    }

    /// Returns a sub-reporter covering slot `idx` of `total` equal partitions.
    /// Equivalent to `scaled(idx/total*100, (idx+1)/total*100)`.
    pub fn step(&self, idx: usize, total: usize) -> ProgressReporter {
        let total = total.max(1) as f64;
        let start = idx as f64 / total * 100.0;
        let end = (idx + 1) as f64 / total * 100.0;
        self.scaled(start, end)
    }
}

pub trait IntoVec<T> {
    fn into_vec<U>(self) -> Vec<U>
    where
        T: Into<U>;
}

impl<T> IntoVec<T> for Vec<T> {
    fn into_vec<U>(self) -> Vec<U>
    where
        T: Into<U>,
    {
        self.into_iter()
            .map(|x| x.into())
            .collect()
    }
}

/// `CREATE_NO_WINDOW` — spawn background tools without a console window on
/// Windows. Without it, every ffmpeg/ffprobe/yt-dlp child (transcoding, seeking,
/// subtitle extraction, probing) pops a cmd window on the user's desktop.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Hide the Windows console for a child process spawned with
/// [`std::process::Command`] or [`tokio::process::Command`]. No-op on non-Windows.
pub trait HideConsole {
    fn hide_console(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl HideConsole for std::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(windows)]
impl HideConsole for tokio::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

#[cfg(not(windows))]
impl HideConsole for std::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        self
    }
}

#[cfg(not(windows))]
impl HideConsole for tokio::process::Command {
    fn hide_console(&mut self) -> &mut Self {
        self
    }
}
