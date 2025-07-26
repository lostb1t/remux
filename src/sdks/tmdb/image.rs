use crate::sdks::core::endpoint::Endpoint;
use http::Method;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::MediaType;

#[derive(Debug, Serialize, Clone)]
pub struct Image {
    #[serde(skip)]
    pub media_type: MediaType,
    #[serde(skip)]
    pub id: u32,
    pub include_image_language: Option<String>, // e.g. "null" or "en,null"
}

impl Endpoint for Image {
    type Output = ImagesResponse;

    fn method(&self) -> Method {
        Method::GET
    }

    fn endpoint(&self) -> String {
        format!("/{}/{}/images", self.media_type, self.id)
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
      self.into()
    }
}


#[derive(Debug, Deserialize, Clone)]
pub struct ImageInfo {
    pub aspect_ratio: f32,
    pub height: u32,
    pub width: u32,
    pub file_path: String,
    pub iso_639_1: Option<String>,
    pub vote_average: Option<f32>,
    pub vote_count: Option<u32>,
}

impl ImageInfo {
    pub fn url(&self, size: &str) -> String {
        format!("https://image.tmdb.org/t/p/{}/{}", size, self.file_path)
    }
}


#[derive(Debug, Deserialize, Clone)]
pub struct ImagesResponse {
    pub backdrops: Vec<ImageInfo>,
    pub posters: Vec<ImageInfo>,
    pub logos: Vec<ImageInfo>,
}