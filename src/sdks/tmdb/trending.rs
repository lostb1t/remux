use serde::{Deserialize, Deserializer, Serialize};

use anyhow::Result;
use bon::Builder;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

//use crate::sdks;
//use crate::sdks::core::Endpoint;
//use crate::clients::core::RestClient;
use crate::media::Media;

#[derive(Debug, EnumString, EnumDisplay, Clone)]
#[strum(serialize_all = "lowercase")]
pub enum MediaType {
    Movie,
    Tv,
    People,
    All,
}

impl Default for MediaType {
    fn default() -> Self {
        MediaType::All
    }
}

#[derive(Debug, EnumString, EnumDisplay, Clone)]
#[strum(serialize_all = "lowercase")]
pub enum TimeWindow {
    Day,
    Week,
}

impl Default for TimeWindow {
    fn default() -> Self {
        TimeWindow::Week
    }
}

#[derive(Debug, Builder, Clone)]
pub struct Trending {
    #[builder(default)]
    time_window: TimeWindow,
    #[builder(default = 1)]
    page: u32,
    #[builder(default)]
    media_type: MediaType,
}

impl super::super::core::Endpoint for Trending {
    type Output = super::PaginatedResult<super::MediaShort>;

    fn endpoint(&self) -> String {
        format!(
            "trending/{}/{}",
            self.media_type.clone(),
            self.time_window.clone()
        )
        .to_string()
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
        let mut params = super::super::core::QueryParams::default();
        params.push("page", self.page.clone());
        //params.push("sort_by", self.sort_by.clone().to_string());
        params
    }
}

impl super::super::core::Pageable for Trending {
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
