use serde::{Deserialize, Deserializer, Serialize};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    #[serde(rename = "MediaContainer")]
    pub media_container: MediaContainer,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaContainer {
    pub size: u32,
    pub total_size: u32,
    pub offset: u32,
    pub allow_sync: bool,
    pub identifier: String,
    pub media_tag_prefix: String,
    pub media_tag_version: i64,
    #[serde(rename = "Metadata", skip_serializing_if = "Vec::is_empty", default)]
    pub metadata: Vec<Metadata>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub rating_key: String,
    pub key: String,
    pub guid: String,
    pub studio: Option<String>,
    #[serde(rename = "type")]
    pub media_type: String,
    pub title: String,
    pub library_section_title: Option<String>,
    #[serde(rename = "librarySectionID")]
    pub library_section_id: Option<i64>,
    pub library_section_key: Option<String>,
    pub summary: Option<String>,
    pub index: Option<i64>,
    pub audience_rating: Option<f64>,
    pub year: Option<u64>,
    pub thumb: Option<String>,
    pub art: Option<String>,
    pub duration: Option<i64>,
    pub originally_available_at: Option<String>,
    pub leaf_count: Option<i64>,
    pub viewed_leaf_count: Option<i64>,
    pub child_count: Option<i64>,
    pub added_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub audience_rating_image: Option<String>,
    #[serde(rename = "Genre", default, skip_serializing_if = "Vec::is_empty")]
    pub genre: Vec<Genre>,
    #[serde(rename = "Country", default, skip_serializing_if = "Vec::is_empty")]
    pub country: Vec<Country>,
    #[serde(rename = "Role", default, skip_serializing_if = "Vec::is_empty")]
    pub role: Vec<Role>,
    pub title_sort: Option<String>,
    pub content_rating: Option<String>,
    pub rating: Option<f64>,
    pub skip_count: Option<i64>,
    pub tagline: Option<String>,
    pub chapter_source: Option<String>,
    pub primary_extra_key: Option<String>,
    pub rating_image: Option<String>,
    #[serde(rename = "Media")]
    #[serde(default)]
    pub media: Vec<Medum>,
    #[serde(rename = "Director")]
    #[serde(default)]
    pub director: Vec<Director>,
    #[serde(rename = "Writer")]
    #[serde(default)]
    pub writer: Vec<Writer>,
    pub original_title: Option<String>,
    #[serde(rename = "Collection")]
    #[serde(default)]
    pub collection: Vec<Collection>,
    pub theme: Option<String>,
    pub edition_title: Option<String>,
    #[serde(rename = "Guid")]
    #[serde(default)]
    pub guids: Vec<Guid>,
}

impl Metadata {
    pub fn get_tmdb_id(&self) -> Option<u32> {
        for guid in self.guids.clone() {
            if guid.id.starts_with("tmdb") {
                let results: Vec<&str> = guid.id.split("://").collect();
                return Some(results[1].parse().unwrap());
            }
        }
        None
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guid {
    pub id: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Country {
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Medum {
    pub id: i64,
    pub duration: Option<i64>,
    pub bitrate: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub aspect_ratio: Option<f64>,
    pub audio_channels: Option<i64>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub video_resolution: Option<String>,
    pub container: Option<String>,
    pub video_frame_rate: Option<String>,
    pub video_profile: Option<String>,
    #[serde(rename = "Part")]
    pub part: Vec<Part>,
    pub audio_profile: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: i64,
    pub key: String,
    pub duration: Option<i64>,
    pub file: String,
    pub size: i64,
    pub container: Option<String>,
    pub video_profile: Option<String>,
    pub audio_profile: Option<String>,
    pub has_thumbnail: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Director {
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Writer {
    pub tag: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub tag: String,
}

#[derive(Copy, Serialize, Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Movie,
    Tv,
    Season,
    Episode,
}

impl MediaType {
    pub fn value(&self) -> u64 {
        match *self {
            MediaType::Movie => 1,
            MediaType::Tv => 2,
            MediaType::Season => 3,
            MediaType::Episode => 4,
        }
    }
}
