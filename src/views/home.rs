use std::time::Duration;

use crate::sdks;
use crate::components;
use dioxus::prelude::*;
use eyre::Result;
// use jellyfin_api;
//use daisy_rsx;
use dioxus_logger::tracing::{debug, info};

//use remux_web::{hooks::*};
use tokio_with_wasm::alias as tokio;




#[component]
pub fn Home() -> Element {
    //let tmdb_client = use_tmdb_client();
  
    
    rsx! {
        // components::Button { "Click me" }
        //Media {}
        components::Hero { }
        components::MediaList {
          title: Some("Trending".to_string()),
          items: vec![]
        }
    }
}