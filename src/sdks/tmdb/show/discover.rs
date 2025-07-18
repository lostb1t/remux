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
pub struct ShowDiscover {
    #[builder(default)]
    sort_by: SortOptions,
    #[builder(default = "1")]
    page: u32,
}

impl ShowDiscover {
    /// Create a builder for the endpoint.
    pub fn builder() -> ShowDiscoverBuilder {
        ShowDiscoverBuilder::default()
    }
}

impl crate::sdks::core::Endpoint for ShowDiscover {
    type Output = crate::sdks::tmdb::PaginatedResult<super::ShowShort>;

    fn endpoint(&self) -> String {
        "discover/tv".to_string()
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
        let mut params = crate::sdks::core::QueryParams::default();
        params.push("page", self.page.clone());
        params.push("sort_by", self.sort_by.clone().to_string());
        params
    }
}

impl crate::sdks::core::Pageable for ShowDiscover {
    // type PageOutput = TryInto<super::ShowShort>;
    // type Item = super::ShowShort;

    fn set_page(&mut self, page: u32) -> &mut Self {
        self.page = page;
        self
    }
    // fn get_page(&self) -> u32 {
    //     self.page
    // }

    // async fn paged_query<T>(&self, client: &RestClient) -> Result<Vec<T>> {
}
