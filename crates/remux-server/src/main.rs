#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use clap::Parser;
use remux_server::{FilesystemPaths, load_config_from_env, serve, setup_logging};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Parser)]
#[command(about = "Remux media server")]
struct Cli {
    #[arg(long, help = "Data directory")]
    datadir: Option<PathBuf>,
    #[arg(long, help = "Bind address")]
    host: Option<std::net::IpAddr>,
    #[arg(long, help = "HTTP port")]
    port: Option<u16>,
    #[arg(long, help = "SQLite database URL")]
    database_url: Option<String>,
    #[arg(long, help = "Path to ffmpeg binary")]
    ffmpeg: Option<PathBuf>,
    #[arg(long, help = "Path to ffprobe binary")]
    ffprobe: Option<PathBuf>,
    #[arg(
        long,
        help = "OTLP gRPC endpoint for tracing spans (e.g. http://jaeger:4317)"
    )]
    otlp_endpoint: Option<String>,
}

fn load_paths() -> FilesystemPaths {
    FilesystemPaths::load_from_env()
}

const LOG_RETENTION: Duration = Duration::from_secs(5 * 24 * 60 * 60);

fn cleanup_old_logs(dir: &Path) {
    let cutoff = SystemTime::now()
        .checked_sub(LOG_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("remux.") && name.ends_with(".log") {
            if let Ok(metadata) = entry.metadata() {
                if metadata
                    .modified()
                    .map(|modified| modified < cutoff)
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Bootstrap ffmpeg paths before Config loads (they're read as bare env vars).
    if let Some(p) = &cli.ffmpeg {
        unsafe { std::env::set_var("FFMPEG_PATH", p) };
    }
    if let Some(p) = &cli.ffprobe {
        unsafe { std::env::set_var("FFPROBE_PATH", p) };
    }

    let mut config = load_config_from_env()?;

    // CLI args win over env.
    if let Some(v) = cli.datadir {
        config.data_dir = v;
    }
    if let Some(v) = cli.host {
        config.host = v;
    }
    if let Some(v) = cli.port {
        config.port = v;
    }
    if let Some(v) = cli.database_url {
        config.database_url = Some(v);
    }
    if let Some(v) = cli.otlp_endpoint {
        config.otlp_endpoint = Some(v);
    }

    // Needs config loaded first — the OTLP endpoint (if any) lives there.
    let config = config.resolve();
    let log_dir = config
        .data_dir
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;
    cleanup_old_logs(&log_dir);
    setup_logging(
        Some(&log_dir),
        config
            .otlp_endpoint
            .as_deref(),
    );

    let cleanup_dir = log_dir.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
            cleanup_old_logs(&cleanup_dir);
        }
    });

    serve(config, load_paths()).await
}

#[cfg(test)]
mod tests {
    use super::cleanup_old_logs;
    use remux_server::load_config;
    use std::time::{Duration, SystemTime};

    #[test]
    fn cleanup_removes_only_old_remux_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale_log = dir
            .path()
            .join("remux.2020-01-01.log");
        let unrelated = dir
            .path()
            .join("other-2020-01-01.log");
        let fresh_log = dir
            .path()
            .join("remux.2026-10-28.log");
        std::fs::write(&stale_log, "old").unwrap();
        std::fs::write(&unrelated, "keep").unwrap();
        std::fs::write(&fresh_log, "keep").unwrap();
        let old = SystemTime::now() - Duration::from_secs(6 * 24 * 60 * 60);
        let old_times = std::fs::FileTimes::new().set_modified(old);
        std::fs::File::open(&stale_log)
            .unwrap()
            .set_times(old_times)
            .unwrap();
        std::fs::File::open(&unrelated)
            .unwrap()
            .set_times(old_times)
            .unwrap();

        cleanup_old_logs(dir.path());

        assert!(!stale_log.exists());
        assert!(unrelated.exists());
        assert!(fresh_log.exists());
    }

    #[test]
    fn parses_port_from_string_environment_value() {
        let env = config::Environment::default().source(Some({
            let mut env = config::Map::new();
            env.insert("PORT".into(), "5000".into());
            env
        }));

        let config = load_config(env).unwrap();

        assert_eq!(config.port, 5000);
    }

    #[test]
    fn parses_host_from_environment_value() {
        let env = config::Environment::default().source(Some({
            let mut env = config::Map::new();
            env.insert("HOST".into(), "::".into());
            env
        }));

        let config = load_config(env).unwrap();

        assert_eq!(
            config.host,
            std::net::IpAddr::from(std::net::Ipv6Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn host_defaults_to_every_ipv4_interface() {
        let config = load_config(
            config::Environment::default().source(Some(config::Map::new())),
        )
        .unwrap();

        assert_eq!(
            config.host,
            std::net::IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED)
        );
    }
}
