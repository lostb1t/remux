#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();
    dotenvy::dotenv().ok();
    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());
    let cli_config: CliConfig = config::Config::builder()
        .add_source(config::File::with_name(&cfg).required(false))
        .add_source(config::Environment::default())
        .build()?
        .try_deserialize()?;
    serve(cli_config.base, cli_config.paths).await
}
