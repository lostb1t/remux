use crate::{Auth, Body, ClientError, Endpoint, RestClient};
use http::HeaderValue;

pub mod models;
pub use models::*;
pub mod endpoints;
pub use endpoints::*;

#[derive(Clone, Debug)]
pub struct JellyfinAuth {
    pub client: String,
    pub device: String,
    pub device_id: String,
    pub version: String,
    pub token: Option<String>,
}

impl JellyfinAuth {
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            client: "Remux Dashboard".to_string(),
            device: "Browser".to_string(),
            device_id: device_id.into(),
            version: "1.0.0".to_string(),
            token: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

impl Auth for JellyfinAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut val = format!(
            r#"MediaBrowser Client="{}", Device="{}", DeviceId="{}", Version="{}""#,
            self.client, self.device, self.device_id, self.version
        );
        if let Some(token) = &self.token {
            val.push_str(&format!(r#", Token="{}""#, token));
        }
        match HeaderValue::from_str(&val) {
            Ok(v) => req.header(http::header::AUTHORIZATION, v),
            Err(_) => req,
        }
    }
}

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?)
}

pub fn authed_client(
    base: &str,
    device_id: impl Into<String>,
    token: impl Into<String>,
) -> Result<RestClient<JellyfinAuth>, url::ParseError> {
    Ok(
        RestClient::new(base)?
            .with_auth(JellyfinAuth::new(device_id).with_token(token)),
    )
}
