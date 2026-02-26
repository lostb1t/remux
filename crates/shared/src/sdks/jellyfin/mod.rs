pub mod models;
pub use models::*;

use crate::sdks::aio;

impl From<aio::MediaType> for MediaType {
    fn from(kind: aio::MediaType) -> Self {
        match kind {
            aio::MediaType::Movie => MediaType::Movie,
            aio::MediaType::Series | aio::MediaType::Tv => MediaType::Series,
            _ => MediaType::Unknown,
        }
    }
}

impl From<MediaType> for aio::MediaType {
    fn from(kind: MediaType) -> Self {
        match kind {
            MediaType::Movie => aio::MediaType::Movie,
            MediaType::Series => aio::MediaType::Series,
            _ => aio::MediaType::Movie,
        }
    }
}
