use crate::media;
use crate::sdks::core::{CommaSeparatedList, Endpoint, QueryParams};
use bon::Builder;
use http::{header, HeaderMap, Method, Request};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

use super::{BaseItemDto, ItemType};

#[derive(Builder, Default, Debug, Clone)]
pub struct GenreEndpoint {
    #[builder(default = 25)]
    pub limit: u32,

    #[builder(default = 0)]
    pub start_index: u32,
}

impl Endpoint for GenreEndpoint {
    type Output = super::PaginatedResult<super::BaseItemDto>;

    fn endpoint(&self) -> String {
        "Genres".to_string()
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();

        params
            .push("Limit", self.limit)
            .push("StartIndex", self.start_index);

        params
    }
}
