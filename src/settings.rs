use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info};
use dioxus_storage::use_persistent;
use dioxus_storage::{use_synced_storage, LocalStorage};
use serde::{Deserialize, Serialize};

use crate::components;
use crate::media;
use crate::server;
//use crate::addons::{Addon};

const SETTINGS_KEY: &str = "settings";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Addon {
    pub name: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Setting {
    pub value: String,
    pub locked: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Settings {
    pub addons: Vec<Addon>,
    pub catalogs: Vec<crate::media::Media>,
    pub version_auto_select: bool,
    pub filter_watched: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            addons: Vec::new(),
            catalogs: Vec::new(),
            version_auto_select: false,
            filter_watched: true,
        }
    }
}

impl Settings {
    // pub fn merge(&self, other: &Settings) -> Settings {
    //     Settings {
    //         addons: {
    //             let mut merged = self.addons.clone();
    //             merged.extend(other.addons.iter().cloned());
    //             merged
    //         },
    //         catalogs: {
    //             let mut merged = self.catalogs.clone();
    //             merged.extend(other.catalogs.iter().cloned());
    //             merged
    //         },
    //         // version_auto_select: other.version_auto_select,
    //     }
    // }

    pub fn add_catalogs(mut self, other: Vec<media::Media>) -> Self {
        for other_cat in &other {
            if let None = self.catalogs.iter_mut().find(|c| c.id == other_cat.id) {
                self.catalogs.push(other_cat.clone());
            }
        }
        self.clone()
    }

    pub fn into_catalogs(
        &self,
        server_catalogs: Vec<crate::media::Media>,
    ) -> Vec<crate::media::Media> {
        ///server.get_collections().await
        // let settings_catalogs = settings.read().catalogs.clone();
        server_catalogs
            .iter()
            .filter_map(|col| {
                let setting = self.catalogs.iter().find(|c| c.id == col.id);

                Some(crate::media::Media {
                    id: col.id.clone(),
                    title: setting
                        .map(|s| s.title.clone())
                        .unwrap_or(col.title.clone()),
                    enabled: setting.map(|s| s.enabled).unwrap_or(col.enabled),
                    card_variant: setting
                        .map(|s| s.card_variant.clone())
                        .unwrap_or_else(|| col.card_variant.clone()),
                    ..col.clone()
                })
            })
            .collect()
    }
}

pub fn use_settings() -> Signal<Settings> {
    // use_persistent("settings", || Settings::default())
    use_synced_storage::<LocalStorage, Settings>(SETTINGS_KEY.to_string(), || Settings::default())
}
