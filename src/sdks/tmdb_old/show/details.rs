use serde::{Deserialize, Deserializer, Serialize};

use derive_builder::Builder;
use itertools::Itertools;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
pub struct ShowEndpoint {
    id: u32,
    #[builder(default = "Some(\"en\".to_string())")]
    language: Option<String>,
    #[builder(default = "Some(vec![\"images\".to_string(),\"watch/providers\".to_string()])")]
    //#[builder(default)]
    append_to_response: Option<Vec<String>>,
}

impl ShowEndpoint {
    /// Create a builder for the endpoint.
    pub fn builder() -> ShowEndpointBuilder {
        ShowEndpointBuilder::default()
    }
}

impl crate::sdks::core::Endpoint for ShowEndpoint {
    type Output = super::Show;

    fn endpoint(&self) -> String {
        format!("tv/{}", self.id)
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
        let mut params = crate::sdks::core::QueryParams::default();
        // if self.language.is_some() {
        //     params.push("language", self.language.clone().unwrap());
        // }
        if self.append_to_response.is_some() {
            params.push(
                "append_to_response",
                self.append_to_response.clone().unwrap().iter().join(","),
            );
        }
        params
    }
}
