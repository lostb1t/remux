use dioxus::prelude::*;
use dioxus_logger::tracing::{info, Level};
//use crate::server::{Server, Servers};
use crate::sdks::tmdb::TmdbClient;

#[derive(Clone, Copy, Default)]
pub struct AppState {
    //pub user: Signal<Option<jellyfin_api::types::AuthenticationResult>>,
}

#[derive(Clone)]
pub struct TmdbClientW(pub TmdbClient);

pub fn use_app() -> AppState {
    use_context::<AppState>()
}

//pub fn use_servers() -> Signal<Servers> {
//    use_context::<Signal<Servers>>()
//}

pub fn use_tmdb_client() -> TmdbClient {
    use_context::<TmdbClientW>().0
}
