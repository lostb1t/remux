use dioxus::{logger::tracing::Level, prelude::*};
use dioxus_use_js::use_js;
use serde::{Deserialize, Deserializer, Serialize};

use_js!("assets/remux.js"::*);

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScrollInfo {
    pub scroll_top: f64,
    pub scroll_left: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    pub client_width: f64,
    pub client_height: f64,
    pub offset_width: f64,
    pub offset_height: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}