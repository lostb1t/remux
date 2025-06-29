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
use serde_with::serde_as;
use serde_with::StringWithSeparator;
use serde_with::formats::{CommaSeparator, SpaceSeparator};
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;


#[derive(Debug, Builder, Clone)]
#[builder(setter(strip_option))]
pub struct ResourceList {
    #[builder(default = "true")]
    pub include_https: bool
}

impl ResourceList {
    pub fn builder() -> ResourceListBuilder {
        ResourceListBuilder::default()
    }
}

impl Endpoint for ResourceList {
    type Output = Vec<Resource>;

    fn endpoint(&self) -> String {
        "resources".to_string()
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();
        params.push("includeHttps", self.include_https);
        params
    }
}

// #[derive(Default, Debug, Clone, Serialize, Deserialize)]
// #[serde(rename_all = "camelCase")]
// pub struct Resources {
//     pub name: String,
//     pub provides: Provider,
//     pub connections: Vec<Connection>
// }

#[serde_as]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
		//#[serde_as(as = "StringWithSeparator::<SpaceSeparator, Provides>")]
    //pub provides: Vec<Provides>,
		pub provides: String,
    pub connections: Vec<Connection>,
		pub access_token: Option<String>,
		pub client_identifier: String,
}

impl Resource {
	pub fn is_server(&self) -> bool {
    self.provides.contains("server")
	}
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub uri: String,
    pub local: bool,
}

#[derive(Debug, PartialEq, EnumString, EnumDisplay, Clone, Serialize, Deserialize)]
#[strum(serialize_all = "lowercase")]
pub enum Provides {
    #[serde(alias = "server")]
    Server,
    Unknown
}

impl Default for Provides {
    fn default() -> Self {
        Provides::Unknown
    }
}

