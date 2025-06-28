use std::time::Duration;

use crate::clients;
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
    rsx! {
        // components::Button { "Click me" }
        //Media {}
        components::Hero { }
        components::MediaRow {}
    }
}