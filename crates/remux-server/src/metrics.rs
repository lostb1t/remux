//! Per-endpoint server-side latency metrics.
//!
//! A single [`tower`] middleware ([`track`]) keyed on the axum route *template*
//! ([`MatchedPath`]) records the handler latency of every registered route from
//! one insertion point, so all ~300 endpoints are covered without touching any
//! handler. Collection is gated behind [`crate::Config::metrics_enabled`]
//! (default `false`): when off, the middleware is a single bool read + branch,
//! so production pays effectively nothing.
//!
//! The middleware times only up to when the handler returns its [`Response`]
//! (headers + body handle) — it never polls or buffers the body — so streaming
//! routes (HLS, `/audio/{id}/universal`, WebSocket upgrades) record
//! handler-completion latency (time-to-first-byte), not full-transfer time, and
//! are never forced to buffer.
//!
//! GET/HEAD are classified as reads; POST/PUT/PATCH/DELETE as mutations (which
//! serialise on the single-writer `DB_WRITE_SEMAPHORE`), so the two can be
//! analysed separately in the snapshot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{MatchedPath, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;

use crate::AppState;

static HEALTHY_SAMPLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LAST_RETENTION_SWEEP_MS: AtomicU64 = AtomicU64::new(0);

/// Sub-buckets per power of two (HdrHistogram-style). Plain log2 buckets bound
/// the reported percentile only to within a factor of two — a 6.7 s sample would
/// be reported as "4.2 s" — which is far too coarse to compare two runs.
/// Splitting each octave into `2^SUB_BITS` linear sub-buckets bounds the
/// relative error to ~1/2^SUB_BITS (≈6% here) while staying allocation-free.
const SUB_BITS: u32 = 3;
const SUB_COUNT: usize = 1 << SUB_BITS; // 8

/// Values below this are counted exactly (one bucket per microsecond).
const LINEAR_LIMIT: u64 = (SUB_COUNT as u64) << 1; // 16

/// Enough buckets for the linear region plus octaves up to ~2^35 µs (≈9.5 h).
const NUM_BUCKETS: usize = 256;

/// Fixed-size array of atomic bucket counters. A newtype is needed because
/// `[AtomicU64; N]` cannot be `#[derive(Default)]`-constructed directly.
struct Buckets([AtomicU64; NUM_BUCKETS]);

impl Default for Buckets {
    fn default() -> Self {
        Buckets(std::array::from_fn(|_| AtomicU64::new(0)))
    }
}

/// Accumulated latency statistics for a single `(method, template)` route.
///
/// All counters are plain atomics updated with `Relaxed` ordering: the values
/// are monotonic aggregates read only by the snapshot endpoint, so no
/// happens-before relationship between them is required.
#[derive(Default)]
pub struct RouteStat {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
    buckets: Buckets,
}

/// Bucket index for a microsecond sample: exact below [`LINEAR_LIMIT`], then
/// `SUB_COUNT` linear sub-buckets per octave.
#[inline]
fn bucket_index(us: u64) -> usize {
    if us < LINEAR_LIMIT {
        return us as usize;
    }
    let octave = (63 - us.leading_zeros()) as usize; // floor(log2 us), >= SUB_BITS+1
    let sub = ((us >> (octave - SUB_BITS as usize)) & (SUB_COUNT as u64 - 1)) as usize;
    let idx =
        LINEAR_LIMIT as usize + (octave - (SUB_BITS as usize + 1)) * SUB_COUNT + sub;
    idx.min(NUM_BUCKETS - 1)
}

/// Inclusive upper bound (µs) of the values that land in `bucket`. Used as the
/// reported percentile value, so a percentile is never under-reported.
#[inline]
fn bucket_upper_us(bucket: usize) -> u64 {
    if (bucket as u64) < LINEAR_LIMIT {
        return bucket as u64;
    }
    let rel = bucket - LINEAR_LIMIT as usize;
    let octave = SUB_BITS as usize + 1 + rel / SUB_COUNT;
    let sub = rel % SUB_COUNT;
    let shift = octave - SUB_BITS as usize;
    // Values in this sub-bucket are [(SUB_COUNT+sub) << shift, +(1<<shift)).
    (((SUB_COUNT + sub) as u64) << shift) + (1u64 << shift) - 1
}

impl RouteStat {
    #[inline]
    fn record(&self, us: u64, status: u16) {
        self.count
            .fetch_add(1, Ordering::Relaxed);
        self.total_us
            .fetch_add(us, Ordering::Relaxed);
        self.buckets
            .0[bucket_index(us)]
        .fetch_add(1, Ordering::Relaxed);
        // Saturating max via CAS loop (contention here is negligible — one op
        // per request and the value only ever grows).
        let mut cur = self
            .max_us
            .load(Ordering::Relaxed);
        while us > cur {
            match self
                .max_us
                .compare_exchange_weak(cur, us, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
        if (500..600).contains(&status) {
            self.status_5xx
                .fetch_add(1, Ordering::Relaxed);
        } else if (400..500).contains(&status) {
            self.status_4xx
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Approximate percentile in microseconds, reported as the upper bound of
    /// the bucket the target rank falls into. `p` is a fraction in `[0, 1]`.
    fn percentile_us(&self, p: f64) -> u64 {
        let total = self
            .count
            .load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let target = (total as f64 * p).ceil() as u64;
        let mut cum = 0u64;
        for (i, b) in self
            .buckets
            .0
            .iter()
            .enumerate()
        {
            cum += b.load(Ordering::Relaxed);
            if cum >= target {
                return bucket_upper_us(i);
            }
        }
        self.max_us
            .load(Ordering::Relaxed)
    }
}

/// Cloneable handle to the process-wide route metrics table. Cheap to clone
/// (one `Arc`). Lives on [`crate::AppContext`].
#[derive(Clone, Default)]
pub struct Metrics {
    // Keyed by "METHOD TEMPLATE" (e.g. "GET /items/{id}"). The outer lock is
    // taken for read on every recorded request and for write only the first
    // time a given route is seen (cold path), so steady-state contention is a
    // shared read lock plus a handful of relaxed atomics.
    inner: Arc<RwLock<HashMap<String, Arc<RouteStat>>>>,
}

impl Metrics {
    /// Record one completed request. `template` is the matched route template
    /// (not the concrete URI), so `/items/1` and `/items/2` aggregate together.
    pub fn record(
        &self,
        method: &'static str,
        template: &str,
        elapsed: std::time::Duration,
        status: u16,
    ) {
        let us = elapsed
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let key = format!("{method} {template}");
        // Fast path: route already present -> shared read lock only.
        {
            let map = self
                .inner
                .read()
                .unwrap();
            if let Some(stat) = map.get(&key) {
                stat.record(us, status);
                return;
            }
        }
        // Cold path: first sighting of this route.
        let stat = {
            let mut map = self
                .inner
                .write()
                .unwrap();
            map.entry(key)
                .or_insert_with(|| Arc::new(RouteStat::default()))
                .clone()
        };
        stat.record(us, status);
    }

    /// Point-in-time snapshot of every route seen so far, sorted by total time
    /// spent (descending) so the biggest aggregate cost surfaces first.
    pub fn snapshot(&self) -> Vec<RouteSnapshot> {
        let map = self
            .inner
            .read()
            .unwrap();
        let mut rows: Vec<RouteSnapshot> = map
            .iter()
            .map(|(key, stat)| {
                let (method, template) = key
                    .split_once(' ')
                    .unwrap_or(("", key.as_str()));
                let count = stat
                    .count
                    .load(Ordering::Relaxed);
                let total_us = stat
                    .total_us
                    .load(Ordering::Relaxed);
                let mean_us = if count > 0 { total_us / count } else { 0 };
                RouteSnapshot {
                    method: method.to_string(),
                    template: template.to_string(),
                    mutation: is_mutation(method),
                    count,
                    total_ms: us_to_ms(total_us),
                    mean_ms: us_to_ms(mean_us),
                    p50_ms: us_to_ms(stat.percentile_us(0.50)),
                    p95_ms: us_to_ms(stat.percentile_us(0.95)),
                    max_ms: us_to_ms(
                        stat.max_us
                            .load(Ordering::Relaxed),
                    ),
                    status_4xx: stat
                        .status_4xx
                        .load(Ordering::Relaxed),
                    status_5xx: stat
                        .status_5xx
                        .load(Ordering::Relaxed),
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            b.total_ms
                .partial_cmp(&a.total_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows
    }
}

#[inline]
fn us_to_ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

#[inline]
fn is_mutation(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Map a request method to a `'static` label. Non-standard methods collapse to
/// `"OTHER"` so the key type can stay `&'static str` with no allocation.
#[inline]
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

/// One route's snapshot row. Serialised by the `/remux/metrics` endpoint.
#[derive(Debug, Serialize)]
pub struct RouteSnapshot {
    pub method: String,
    pub template: String,
    /// `true` for POST/PUT/PATCH/DELETE (write-serialised), `false` for reads.
    pub mutation: bool,
    pub count: u64,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub status_4xx: u64,
    pub status_5xx: u64,
}

/// Middleware that records per-route handler latency when metrics are enabled.
///
/// Inserted via [`axum::middleware::from_fn_with_state`]. `MatchedPath` is
/// populated by axum during routing before this layer's inner service runs, so
/// the route template is available here. Unmatched requests (the web-client
/// fallback) carry no `MatchedPath` and are skipped.
pub async fn track(
    State(state): State<AppState>,
    matched: Option<MatchedPath>,
    req: Request,
    next: Next,
) -> Response {
    let legacy_enabled = state
        .ctx
        .config
        .metrics_enabled;
    let telemetry_enabled = state
        .ctx
        .config
        .telemetry_enabled;
    if !legacy_enabled && !telemetry_enabled {
        return next
            .run(req)
            .await;
    }
    let method = method_label(req.method());
    let request_path = req
        .uri()
        .path()
        .to_string();
    let client_context = client_context(req.headers());
    let template = matched.map(|m| {
        m.as_str()
            .to_string()
    });
    let started = Instant::now();
    let response = next
        .run(req)
        .await;
    if let Some(template) = template {
        let elapsed = started.elapsed();
        let status = response
            .status()
            .as_u16();
        if legacy_enabled {
            state
                .ctx
                .metrics
                .record(method, &template, elapsed, status);
        }
        if telemetry_enabled {
            let latency_ms = elapsed.as_secs_f64() * 1_000.0;
            let slow = latency_ms
                >= state
                    .ctx
                    .config
                    .telemetry_slow_request_ms as f64;
            let failed = status >= 400;
            let sample_rate = state
                .ctx
                .config
                .telemetry_sample_rate
                .clamp(0.0, 1.0);
            let sampled = (HEALTHY_SAMPLE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                % 10_000)
                < (sample_rate * 10_000.0).round() as u64;
            if failed || slow || sampled {
                let db = state
                    .ctx
                    .db
                    .clone();
                let reason = if failed {
                    "error"
                } else if slow {
                    "slow"
                } else {
                    "sample"
                };
                let item_id = item_id_from_path(&request_path);
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "INSERT INTO telemetry_request_events \
                         (method, route_template, status, latency_ms, sample_reason, device_id, device_name, client_name, client_version, item_id, error_category) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                    )
                    .bind(method)
                    .bind(template)
                    .bind(status as i64)
                    .bind(latency_ms)
                    .bind(reason)
                    .bind(client_context.device_id)
                    .bind(client_context.device_name)
                    .bind(client_context.client_name)
                    .bind(client_context.client_version)
                    .bind(item_id)
                    .bind(if failed { Some(format!("http-{status}")) } else { None })
                    .execute(&db)
                    .await;
                });
            }
            schedule_telemetry_retention(
                &state
                    .ctx
                    .db,
            );
        }
    }
    response
}

/// Retention is deliberately opportunistic and hourly: no extra scheduler is
/// required, and a quiet server simply performs the cleanup on its next hit.
fn schedule_telemetry_retention(db: &sqlx::SqlitePool) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let previous = LAST_RETENTION_SWEEP_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(previous) < 60 * 60 * 1_000
        || LAST_RETENTION_SWEEP_MS
            .compare_exchange(previous, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let db = db.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("DELETE FROM telemetry_request_events WHERE created_at < datetime('now', '-30 days')").execute(&db).await;
        let _ = sqlx::query("DELETE FROM telemetry_playback_events WHERE created_at < datetime('now', '-30 days')").execute(&db).await;
    });
}

#[derive(Default)]
struct ClientContext {
    device_id: Option<String>,
    device_name: Option<String>,
    client_name: Option<String>,
    client_version: Option<String>,
}

fn client_context(headers: &axum::http::HeaderMap) -> ClientContext {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .or_else(|| headers.get("X-Emby-Authorization"))
        .and_then(|value| {
            value
                .to_str()
                .ok()
        })
        .unwrap_or_default();
    let value_for = |name: &str| {
        raw.split(',')
            .find_map(|part| {
                let (key, value) = part
                    .trim()
                    .split_once('=')?;
                (key.trim()
                    .eq_ignore_ascii_case(name))
                .then(|| {
                    value
                        .trim()
                        .trim_matches('"')
                        .to_string()
                })
            })
    };
    ClientContext {
        device_id: value_for("DeviceId"),
        device_name: value_for("Device"),
        client_name: value_for("Client"),
        client_version: value_for("Version"),
    }
}

fn item_id_from_path(path: &str) -> Option<String> {
    let segments: Vec<_> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    segments
        .windows(2)
        .find_map(|pair| {
            matches!(
                pair[0]
                    .to_ascii_lowercase()
                    .as_str(),
                "items" | "videos" | "audio"
            )
            .then(|| pair[1].to_string())
        })
}

/// Times an intra-handler scope and logs its duration at `DEBUG` on drop, under
/// the `remux_server::perf` target. When that target/level is disabled (the
/// default in production) the only cost is one `Instant::now()` and an atomic
/// level check — no allocation, no formatting — and it toggles at runtime via
/// the tracing filter (`RUST_LOG=remux_server::perf=debug`) with no rebuild.
///
/// This is the sanctioned way to get a per-function breakdown of a hot path,
/// complementing the per-endpoint [`Metrics`] middleware:
///
/// ```ignore
/// let _p = metrics::perf_span("get_by_filter");
/// // ... work ...
/// // logs `elapsed_us` for "get_by_filter" when the guard drops
/// ```
#[must_use = "the scope is timed until this guard is dropped; bind it to a name"]
pub struct PerfSpan {
    name: &'static str,
    started: Instant,
    enabled: bool,
}

impl PerfSpan {
    fn new(name: &'static str) -> Self {
        let enabled = tracing::enabled!(
            target: "remux_server::perf",
            tracing::Level::DEBUG,
        );
        Self {
            name,
            started: Instant::now(),
            enabled,
        }
    }
}

impl Drop for PerfSpan {
    fn drop(&mut self) {
        if self.enabled {
            tracing::debug!(
                target: "remux_server::perf",
                name = self.name,
                elapsed_us = self
                    .started
                    .elapsed()
                    .as_micros() as u64,
            );
        }
    }
}

/// Start a [`PerfSpan`]; bind the returned guard to time until it drops.
#[inline]
pub fn perf_span(name: &'static str) -> PerfSpan {
    PerfSpan::new(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The emission path must actually fire (name + elapsed_us) when the
    /// `remux_server::perf` target is enabled at DEBUG.
    #[test]
    fn perf_span_emits_when_enabled() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap()
                    .extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_max_level(tracing::Level::DEBUG)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let guard = perf_span("unit_test_scope");
            assert!(guard.enabled, "should be enabled under a DEBUG subscriber");
            drop(guard);
        });

        let out = String::from_utf8(
            buf.0
                .lock()
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert!(out.contains("unit_test_scope"), "missing name: {out}");
        assert!(out.contains("elapsed_us"), "missing elapsed_us: {out}");
    }

    #[test]
    fn perf_span_runs_without_a_subscriber() {
        // No global subscriber in unit tests: the guard must construct and drop
        // cleanly (enabled == false path) without panicking.
        let guard = perf_span("unit_test_scope");
        assert!(!guard.enabled);
        drop(guard);
    }

    #[test]
    fn bucket_index_is_monotonic_and_saturating() {
        // Small values are counted exactly.
        for us in 0..LINEAR_LIMIT {
            assert_eq!(bucket_index(us), us as usize);
        }
        // Monotonic non-decreasing across a wide range.
        let mut prev = 0;
        for us in 0..100_000u64 {
            let b = bucket_index(us);
            assert!(b >= prev, "bucket went backwards at {us}");
            prev = b;
        }
        // Very large samples saturate rather than panicking or overflowing.
        assert_eq!(bucket_index(u64::MAX), NUM_BUCKETS - 1);
    }

    /// The whole point of sub-bucketing: a reported percentile must be close to
    /// the real value, not merely within a factor of two.
    #[test]
    fn bucket_upper_bound_is_tight_and_never_underreports() {
        for us in [
            17u64, 100, 999, 1_500, 40_000, 299_941, 1_000_000, 6_750_000, 14_184_000,
        ] {
            let upper = bucket_upper_us(bucket_index(us));
            assert!(upper >= us, "under-reported {us} as {upper}");
            let err = (upper - us) as f64 / us as f64;
            assert!(err < 0.15, "bucket too coarse for {us}: {upper} ({err:.3})");
        }
    }

    #[test]
    fn records_and_aggregates_per_route() {
        let m = Metrics::default();
        m.record("GET", "/items/{id}", Duration::from_micros(100), 200);
        m.record("GET", "/items/{id}", Duration::from_micros(300), 200);
        m.record("GET", "/items/{id}", Duration::from_micros(500), 404);
        m.record("POST", "/items", Duration::from_millis(2), 500);

        let snap = m.snapshot();
        assert_eq!(snap.len(), 2);

        let items = snap
            .iter()
            .find(|r| r.method == "GET" && r.template == "/items/{id}")
            .unwrap();
        assert_eq!(items.count, 3);
        assert!(!items.mutation);
        assert_eq!(items.status_4xx, 1);
        assert_eq!(items.status_5xx, 0);
        // mean = (100+300+500)/3 = 300 µs = 0.3 ms
        assert!((items.mean_ms - 0.3).abs() < 1e-9);

        let post = snap
            .iter()
            .find(|r| r.method == "POST")
            .unwrap();
        assert!(post.mutation);
        assert_eq!(post.status_5xx, 1);
    }

    #[test]
    fn percentile_zero_when_empty() {
        let stat = RouteStat::default();
        assert_eq!(stat.percentile_us(0.5), 0);
    }
}
