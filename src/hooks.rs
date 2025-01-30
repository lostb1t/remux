use dioxus::prelude::*;
use dioxus_logger::tracing::{info, Level};
use jellyfin_api;
use jellyfin_api::Client;

#[derive(Clone, Copy, Default)]
pub struct AppState {
    pub user: Signal<Option<jellyfin_api::types::AuthenticationResult>>,
}


pub fn use_app() -> AppState {
    use_context::<AppState>()
}

pub fn use_client() -> Client {
    use_context::<Client>()
}