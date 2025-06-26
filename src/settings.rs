use dioxus::prelude::*;
use dioxus_sdk::storage::use_persistent;
use serde::{Deserialize, Serialize};
use dioxus_logger::tracing::{debug, error, info};

//use crate::addons::{Addon};

const SETTINGS_KEY: &str = "settings";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Addon {
    pub name: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Settings {
    pub addons: Vec<Addon>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { addons: vec![] }
    }
}

pub fn use_settings() -> Signal<Settings> {
    use_persistent("settings", || Settings::default())
}