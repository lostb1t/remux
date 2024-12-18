use serde::{Deserialize, Serialize};

// use eyre::Result;
use reqwest;

pub static BASE_API_URL: &str = "http://100.95.68.69:8000/api/v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub id: u64,
    pub name: String,
}

// #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// pub struct MediaeData {
//     #[serde(flatten)]
//     pub item: StoryItem,
//     #[serde(default)]
//     pub comments: Vec<CommentData>,
// }


pub async fn get_media() -> Result<Vec<Media>, reqwest::Error> {
    let url = format!("{}/{}", BASE_API_URL, "media/file");
    let items = reqwest::get(&url).await?.json::<Vec<Media>>().await?;
    Ok(items)
}
