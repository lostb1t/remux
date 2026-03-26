use anyhow::Result;
use remux_server::{Config, serve, setup_logging};

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();
    dotenvy::dotenv().ok();
    let cfg = std::env::var("CONFIG").unwrap_or_else(|_| "/data/config".to_string());
    let config: Config = config::Config::builder()
        .add_source(config::File::with_name(&cfg).required(false))
        .add_source(config::Environment::default())
        .build()?
        .try_deserialize()?;
    serve(config).await
}
