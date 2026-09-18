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
use std::{collections::HashMap, path::Path, pin::Pin};
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
static TMDB_RATE_LIMIT: OnceLock<sdks::SharedRateLimit> = OnceLock::new();
static ADDON_RATE_LIMITS: OnceLock<
    std::sync::Mutex<HashMap<Uuid, sdks::SharedRateLimit>>,
> = OnceLock::new();

/// TMDB clients are built in a few independent paths. They must still share
/// one cooldown, otherwise concurrent metadata refreshes each evade a 429 by
/// constructing their own client.
pub(crate) fn tmdb_rate_limit() -> sdks::SharedRateLimit {
    TMDB_RATE_LIMIT
        .get_or_init(sdks::SharedRateLimit::new)
        .clone()
}

/// Addon clients (see `StremioAddon::service`) are built fresh on every call
/// rather than cached, so without this each concurrent or sequential call to
/// the same addon starts with no memory of a prior 429 — the exact problem
/// `SharedRateLimit` exists to solve, just never wired up per addon.
///
/// Keyed by addon id, not host: two addon configs can point at the same host
/// under different auth/quotas, and must not share a cooldown meant for a
/// different budget. The cost is the mirror case — two configs that really do
/// share one backend account won't coordinate — an acceptable miss since
/// remux has no way to know they share a quota.
pub(crate) fn addon_rate_limit(addon_id: Uuid) -> sdks::SharedRateLimit {
    ADDON_RATE_LIMITS
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .entry(addon_id)
        .or_insert_with(sdks::SharedRateLimit::new)
        .clone()
}

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
                // TMDB does not send Retry-After on 429, so use its short
                // throttle window instead of the SDK's generic 60-second
                // fallback. Must match `addons/tmdb.rs`'s own client factory:
                // both now feed the same shared cooldown, and a 429 seen on
                // either one installs this value as the block duration — two
                // different fallbacks would make the effective cooldown
                // depend on which client happened to see the 429 first.
                .with_default_retry_after(std::time::Duration::from_secs(2))
                .with_shared_rate_limit(tmdb_rate_limit())
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

/// Drives a `ProgressReporter` from a single running item count against a
/// total that can be revised after the fact — unlike `scaled`/`step`, whose
/// bounds are fixed forever at creation. Meant for a task made of several
/// phases (e.g. index refresh, catalog import, metadata refresh) whose
/// individual sizes aren't all known upfront: seed the total with a best
/// guess (or 0), hand out a `child` reporter per phase weighted by its
/// estimated share, and correct the total via `adjust_total` once a phase's
/// real size becomes known — every already-created child keeps working
/// correctly against the corrected total from then on.
#[derive(Clone)]
pub struct ItemProgress {
    reporter: ProgressReporter,
    total: Arc<std::sync::atomic::AtomicI64>,
}

impl ItemProgress {
    pub fn new(reporter: ProgressReporter, initial_total: usize) -> Self {
        Self {
            reporter,
            total: Arc::new(std::sync::atomic::AtomicI64::new(initial_total as i64)),
        }
    }

    /// A child reporter covering `weight` items starting at `base` items
    /// already accounted for elsewhere. When the child reports `pct`, this
    /// maps to `(base + weight * pct/100) / total` against the *current*
    /// total — so a later `adjust_total` call still corrects this child's
    /// contribution to the overall percentage, not just future ones.
    pub fn child(&self, base: usize, weight: usize) -> ProgressReporter {
        let total = self
            .total
            .clone();
        let reporter = self
            .reporter
            .clone();
        ProgressReporter(Arc::new(move |pct: f64| {
            let total = (total
                .load(Ordering::Relaxed)
                .max(1)) as f64;
            let processed = base as f64 + weight as f64 * pct.clamp(0.0, 100.0) / 100.0;
            reporter.set(processed / total * 100.0);
        }))
    }

    /// Correct the total by `delta` (positive or negative) — e.g. replacing
    /// an upfront guess with a phase's real size once it's known. Does not
    /// itself move the displayed percentage; the next report through any
    /// child reflects the corrected total.
    pub fn adjust_total(&self, delta: i64) {
        self.total
            .fetch_add(delta, Ordering::Relaxed);
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

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn reporter() -> (ProgressReporter, Arc<AtomicU64>) {
        let atomic = Arc::new(AtomicU64::new(0));
        (ProgressReporter::new(atomic.clone()), atomic)
    }

    fn read(atomic: &Arc<AtomicU64>) -> f64 {
        f64::from_bits(atomic.load(Ordering::Relaxed))
    }

    #[test]
    fn item_progress_child_reports_weighted_share_of_total() {
        let (root, atomic) = reporter();
        let item_progress = ItemProgress::new(root, 100);
        let child_a = item_progress.child(0, 60);
        let child_b = item_progress.child(60, 40);

        child_a.report(50, 100); // 50% of a 60-item slice = 30 of 100 total.
        assert_eq!(read(&atomic), 30.0);

        child_b.set(100.0); // finishes the remaining 40-item slice.
        assert_eq!(read(&atomic), 100.0);
    }

    #[test]
    fn item_progress_adjust_total_corrects_already_created_children() {
        let (root, atomic) = reporter();
        let item_progress = ItemProgress::new(root, 100);
        let child = item_progress.child(0, 50);

        child.set(100.0);
        assert_eq!(read(&atomic), 50.0, "50 of 100 items done");

        // Total revised upward (e.g. a phase's estimate corrected against its
        // real size) — the *same* child, re-reporting the same percentage of
        // its own slice, must reflect the corrected total, not the one it was
        // created against.
        item_progress.adjust_total(50);
        child.set(100.0);
        assert!(
            (read(&atomic) - 33.3).abs() < 0.2,
            "expected ~33.3%, got {}",
            read(&atomic)
        );
    }
}
