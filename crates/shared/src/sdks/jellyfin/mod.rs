use super::{BasicAuth, ClientError, Endpoint, RestClient};

pub mod models;
pub use models::*;
pub mod endpoints;
pub use endpoints::*;

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?)
}