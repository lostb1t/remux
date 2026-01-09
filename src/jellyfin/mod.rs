use moka::Expiry;
use moka::sync::Cache;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
use crate::db;

pub mod api;
pub mod models;
pub use models::*;


pub fn get_virtual_folders(state: &crate::AppState) -> Vec<BaseItemDto> {
  
    let mut vf = vec![BaseItemDto {
        name: Some("Collections".to_string()),
        //id: "collections".to_string(),
        id: state.config.collection_id,
        //parent_id: Some("test".to_string()),
        //type_: Some(jellyfin::MediaType::CollectionFolder),
        collection_type: Some(CollectionType::Boxsets),
        is_folder: true,
        ..Default::default()
    }];
    
    vf
}