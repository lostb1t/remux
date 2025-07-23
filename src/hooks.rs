use crate::server::{self, Server};
use dioxus::prelude::*;
use dioxus_logger::tracing::{info, Level};
//use crate::sdks::tmdb::TmdbClient;
use crate::capabilities;
use crate::media;
use crate::server::ServerConfig;
use crate::views;
use dioxus_storage::{use_synced_storage, LocalStorage};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use uuid::Uuid;
use whoami;

pub fn use_server() -> Signal<Option<Arc<server::ServerInstance>>> {
    consume_context()
}

pub fn use_caps() -> capabilities::Capabilities {
    consume_context()
}

pub fn use_home_filter() -> views::home::HomeFilter {
    consume_context()
}

use anyhow::anyhow;

pub fn use_genres() -> Vec<media::Genre> {
    consume_context()
}

pub fn use_server_config() -> Signal<Option<ServerConfig>> {
    use_synced_storage::<LocalStorage, Option<ServerConfig>>("server".to_string(), || {
        None::<ServerConfig>
    })
}

//pub fn use_tmdb_client() -> TmdbClient {
//    use_context::<TmdbClientW>().0
//}
