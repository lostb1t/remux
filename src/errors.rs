#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Unauthorized (token expired?)")]
    Unauthorized,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("Other error: {0}")]
    Other(String),
}
