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

// static COUNT: GlobalSignal<i32> = Global::new(|| 0);

#[derive(Clone, Copy, Default)]
pub struct AppState {
    pub is_touch: bool,
    //pub user: Signal<Option<jellyfin_api::types::AuthenticationResult>>,
}

//#[derive(Clone)]
//pub struct TmdbClientW(pub TmdbClient);

pub fn use_app() -> Signal<AppState> {
    consume_context()
}

//pub fn use_top_nav() -> Signal<Vec<views::TopNavItem>> {
//    consume_context()

//}

pub fn use_server_old() -> (
    Signal<Option<Arc<dyn Server>>>,
    Rc<RefCell<dyn FnMut(Arc<dyn Server>)>>,
) {
    let config =
        use_synced_storage::<LocalStorage, _>("server".to_string(), || None::<ServerConfig>);
    let server = use_context_provider(|| Signal::new(None::<Arc<dyn Server>>));
    // let server =   use_context::<AppState>()
    //dbg!(server.value);
    use_future({
        let mut config = config.clone();
        let mut server = server.clone();
        move || async move {
            if let Some(cfg) = config.read().clone() {
                server.set(Some(cfg.into_server().into()));
            }
        }
    });
    let set_server = Rc::new(RefCell::new({
        let mut config = config.clone();
        let mut server = server.clone();
        move |srv: Arc<dyn Server>| {
            config.set(Some(srv.into_config()));
            server.set(Some(srv));
        }
    }));

    (server, set_server)
}

pub fn use_server() -> Signal<Option<Arc<dyn Server>>> {
    consume_context()
}

pub fn use_caps() -> capabilities::Capabilities {
    consume_context()
}

pub fn consume_server() -> anyhow::Result<Arc<dyn Server>> {
    let signal = consume_context::<Signal<Option<Arc<dyn Server>>>>();
    let x = signal
        .peek()
        .clone()
        .ok_or_else(|| anyhow!("No server set"));
    x
    //Ok(server)
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
