use serde::{Deserialize, Deserializer, Serialize};

use anyhow::Result;
use derive_builder::Builder;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

use crate::sdks;
use crate::sdks::core::Endpoint;
use crate::sdks::core::RestClient;
use crate::media::Media;

#[derive(Debug, EnumString, EnumDisplay, Clone)]
pub enum SortOptions {
    #[strum(to_string = "popularity.desc")]
    PopularDesc,
    // #[strum(to_string = "popularity.asc")]
    // PopularAsc,
    // #[strum(to_string = "vote_average.desc")]
    // VoteAverageDesc,
    #[strum(to_string = "vote_average.asc")]
    VoteAverageAsc,
}

impl Default for SortOptions {
    fn default() -> Self {
        SortOptions::PopularDesc
    }
}

#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
pub struct MovieDiscover {
    #[builder(default)]
    sort_by: SortOptions,
    #[builder(default = "1")]
    page: u32,
}

impl MovieDiscover {
    /// Create a builder for the endpoint.
    pub fn builder() -> MovieDiscoverBuilder {
        MovieDiscoverBuilder::default()
    }
}

impl crate::sdks::core::Endpoint for MovieDiscover {
    type Output = crate::sdks::tmdb::PaginatedResult<super::MovieShort>;

    fn endpoint(&self) -> String {
        "discover/movie".to_string()
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
        let mut params = crate::sdks::core::QueryParams::default();
        params.push("page", self.page.clone());
        params.push("sort_by", self.sort_by.clone().to_string());
        params
    }
}

impl crate::sdks::core::Pageable for MovieDiscover {
    // type PageOutput = TryInto<super::MovieShort>;
    //type Item = super::MovieShort;

    fn set_page(&mut self, page: u32) -> &mut Self {
        self.page = page;
        self
    }
    // fn get_page(&self) -> u32 {
    //     self.page
    // }
}
