#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use clap::Parser;
use remux_server::{FilesystemPaths, load_config_from_env, serve, setup_logging};
use std::path::PathBuf;

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
    setup_logging(
        None,
        config
            .otlp_endpoint
            .as_deref(),
    );

    serve(config.resolve(), load_paths()).await
}

#[cfg(test)]
mod tests {
    use remux_server::load_config;

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
