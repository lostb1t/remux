use dioxus::prelude::*;
use dioxus_logger::tracing::{info, Level};
use jellyfin_api;
use jellyfin_api::Client;
use crate::server::{Server, Servers};

#[derive(Clone, Copy, Default)]
pub struct AppState {
    pub user: Signal<Option<jellyfin_api::types::AuthenticationResult>>,
}


pub fn use_app() -> AppState {
    use_context::<AppState>()
}

pub fn use_client() -> Client {
    let mut app = use_app();
    let client = use_context::<Client>();

    // if app.user.read().is_none() {
    if app.user.read().is_none() {
        info!("User is not authenticated");
    } else {
        info!("User is already authenticated");
    }

    client
}

pub fn use_servers() -> Signal<Servers> {
    use_context::<Signal<Servers>>()
}