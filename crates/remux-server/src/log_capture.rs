use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::Subscriber;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer, Registry, reload};

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub level: String,
    pub message: String,
    pub target: String,
    pub timestamp: String,
}

static LOG_TX: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();
static LOG_FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> =
    OnceLock::new();
static LOG_FILE: OnceLock<Mutex<BufWriter<fs::File>>> = OnceLock::new();
static LOG_FILE_PATH: OnceLock<String> = OnceLock::new();

pub struct LogCapture {
    tx: broadcast::Sender<LogLine>,
}

impl LogCapture {
    pub fn new(tx: broadcast::Sender<LogLine>) -> Self {
        Self { tx }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_debug(
        &mut self,
        field: &tracing::field::Field,
        value: &dyn std::fmt::Debug,
    ) {
        if field.name() == "message" {
            // Strip surrounding quotes that Debug adds to strings
            let s = format!("{value:?}");
            self.message = s.trim_matches('"').to_string();
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for LogCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        let level = meta.level().to_string();
        let target = meta.target().to_string();
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let line = LogLine {
            level,
            message: visitor.message,
            target,
            timestamp,
        };
        let json = serde_json::to_string(&line).unwrap_or_default();
        let _ = self.tx.send(line);
        if let Some(mutex) = LOG_FILE.get() {
            if let Ok(mut w) = mutex.lock() {
                let _ = writeln!(w, "{json}");
                let _ = w.flush();
            }
        }
    }
}

fn base_filter() -> String {
    std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info,librqbit_dht=warn,hyper=warn,sqlx=warn".into())
}

pub fn log_file_path() -> Option<&'static str> {
    LOG_FILE_PATH.get().map(String::as_str)
}

pub fn init_file(path: &str) {
    LOG_FILE_PATH.set(path.to_string()).ok();
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => {
            LOG_FILE.set(Mutex::new(BufWriter::new(f))).ok();
        }
        Err(e) => tracing::warn!("could not open log file {path}: {e}"),
    }
}

/// Called once from `setup_logging()`. Initialises globals and returns the
/// layers to be added to the tracing subscriber.
pub fn init() -> (
    reload::Layer<EnvFilter, Registry>,
    LogCapture,
    broadcast::Sender<LogLine>,
) {
    let filter =
        EnvFilter::try_new(base_filter()).unwrap_or_else(|_| EnvFilter::new("info"));
    let (reload_layer, handle) = reload::Layer::new(filter);
    LOG_FILTER_HANDLE.set(handle).ok();

    let (tx, _) = broadcast::channel::<LogLine>(4096);
    LOG_TX.set(tx.clone()).ok();
    let log_capture = LogCapture::new(tx.clone());

    (reload_layer, log_capture, tx)
}

/// Subscribe to the live log stream. Returns `None` if `init()` was never called.
pub fn subscribe() -> Option<broadcast::Receiver<LogLine>> {
    LOG_TX.get().map(|tx| tx.subscribe())
}

/// Change the `remux_server` log level at runtime. Other crates keep their
/// RUST_LOG baseline.
pub fn set_log_level(level: &str) -> Result<()> {
    let handle = LOG_FILTER_HANDLE
        .get()
        .ok_or_else(|| anyhow::anyhow!("log filter handle not initialized"))?;
    let directive = format!("{},remux_server={level}", base_filter());
    handle.modify(|f| *f = EnvFilter::new(&directive))?;
    tracing::info!("log level changed to {level}");
    Ok(())
}
