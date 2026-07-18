#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Result;
use remux_server::{Config, FilesystemPaths, serve, setup_logging};
use serde::Deserialize;

#[derive(Deserialize)]
struct CliConfig {
    #[serde(flatten)]
    base: Config,
    #[serde(flatten)]
    paths: FilesystemPaths,
}

fn load_cli_config(
    cfg: &str,
    env: config::Environment,
) -> Result<CliConfig, config::ConfigError> {
    config::Config::builder()
        .add_source(config::File::with_name(cfg).required(false))
        .add_source(env.try_parsing(true))
        .build()?
        .try_deserialize()
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());
    let cli_config = load_cli_config(&cfg, config::Environment::default())?;
    let base = cli_config
        .base
        .resolve();
    // Logging is initialised after config load so the file appender can target
    // the resolved `log_dir`. The guard must live for the whole process, so it
    // is bound here and only dropped as `main` returns.
    let _log_guard = setup_logging(
        base.log_dir
            .as_deref()
            .map(std::path::Path::new),
    );
    serve(base, cli_config.paths).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_from_string_environment_value() {
        let env = config::Environment::default().source(Some({
            let mut env = config::Map::new();
            env.insert("PORT".into(), "5000".into());
            env
        }));

        let cli_config =
            load_cli_config("/tmp/remux-missing-test-config", env).unwrap();

        assert_eq!(
            cli_config
                .base
                .port,
            5000
        );
    }
}
