use super::super::models;
use crate::clients::core::CommaSeparatedList;
use crate::clients::core::Endpoint;
use crate::clients::core::QueryParams;
use derive_builder::Builder;
use http::HeaderMap;
use http::{header, Method, Request};
use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Builder, Clone)]
#[builder(setter(strip_option))]
pub struct PinCreate {}

impl PinCreate {
    pub fn builder() -> PinCreateBuilder {
        PinCreateBuilder::default()
    }
}

impl Endpoint for PinCreate {
    type Output = PinResponse;

    fn method(&self) -> Method {
        Method::POST
    }

    fn endpoint(&self) -> String {
        "pins".to_string()
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();
        params.push("strong", "true");
        params
    }
}

#[derive(Debug, Builder, Clone)]
#[builder(setter(strip_option))]
pub struct Pin {
    pub id: u64,
    pub code: String,
}

impl Pin {
    pub fn builder() -> PinBuilder {
        PinBuilder::default()
    }
}

impl Endpoint for Pin {
    type Output = PinResponse;

    fn endpoint(&self) -> String {
        format!("pins/{}", self.id.clone())
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();
        params.push("strong", "true");
        params
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinResponse {
    pub id: u64,
    pub qr: String,
    pub auth_token: Option<String>,
    pub product: String,
    pub code: String,
    //pub location: Location,
    pub expires_in: i64,
    pub expires_at: String,
    pub client_identifier: String,
    pub trusted: bool,
    pub created_at: String,
    //pub new_registration: Option<String>,
}
