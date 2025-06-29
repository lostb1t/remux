pub mod library;
pub mod models;
pub mod tv;
pub use models::*;

use crate::clients::core::RestClient;
use once_cell::sync::Lazy;
use uuid::Uuid;

//use crate::clients::core::RestClient;
//use std::cell::OneCell;

pub type PlexTvClient = RestClient;

pub static PLEX_CLIENT_IDENTIFIER: Lazy<String> =
    Lazy::new(|| Uuid::new_v4().to_owned().to_string());
pub static PLEX_PRODUCT: &str = "Remux";
//pub const PLEX_TV_CLIENT: RestClient = RestClient::new("https://plex.tv/api/v2/")
//                .unwrap()
//                .header("x-plex-client-identifier", PLEX_CLIENT_IDENTIFIER);
//}
