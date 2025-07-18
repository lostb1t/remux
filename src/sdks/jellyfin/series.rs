use super::{BaseItemDto, ItemType};
use crate::media;
use crate::sdks::core::{CommaSeparatedList, Endpoint, QueryParams};
use bon::Builder;
use dioxus_logger::tracing::{debug, info};
use http::{header, HeaderMap, Method, Request};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json;
use serde_with::skip_serializing_none;
//use serde_with;
extern crate serde_qs;

#[skip_serializing_none]
#[derive(Builder, Default, Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct NextUpEndpoint {
    pub series_id: String,
    pub user_id: String,

    // pub fields: Option<Vec<String>>,
    #[builder(default = 5)]
    pub limit: u32,
}

impl Endpoint for NextUpEndpoint {
    type Output = super::PaginatedResult<super::BaseItemDto>;

    fn endpoint(&self) -> String {
        "/Shows/NextUp".into()
    }

    fn parameters(&self) -> QueryParams {
        self.into()
    }
}
