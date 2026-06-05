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
    setup_logging();
    dotenvy::dotenv().ok();
    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());
    let cli_config = load_cli_config(&cfg, config::Environment::default())?;
    serve(
        cli_config
            .base
            .resolve(),
        cli_config.paths,
    )
    .await
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
