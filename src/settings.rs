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

#[derive(Serialize, Hash, Eq, Deserialize, Debug, Clone, PartialEq)]
pub struct SettingField<T> {
    pub default: T,
    pub value: Option<T>,
    pub locked: bool,
}


impl<T> SettingField<Vec<T>> {
    pub fn modify_item<F>(&mut self, index: usize, f: F)
    where
        F: FnOnce(&mut T),
    {
        if let Some(vec) = self.value.as_mut() {
            if let Some(item) = vec.get_mut(index) {
                f(item);
            }
        }
    }
}

impl<T: Clone> SettingField<T> {
  
  
    pub fn effective(&self) -> T {
        if self.locked {
            self.default.clone()
        } else {
            self.value.clone().unwrap_or_else(|| self.default.clone())
        }
    }
    
    pub fn set(&mut self, val: T) {
      //debug!(?self.locked);
        if !self.locked {
            self.value = Some(val);
        }
    }
}

impl<T: Default + Clone> Default for SettingField<T> {
    fn default() -> Self {
        Self {
            default: T::default(),
            value: None,
            locked: false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Settings {
    pub addons: SettingField<Vec<Addon>>,
    pub catalogs: SettingField<Vec<crate::media::Media>>,
    pub version_auto_select: SettingField<bool>,
    pub filter_watched: SettingField<bool>,
}

impl Settings {
    pub fn version_auto_select(&self) -> bool {
        self.version_auto_select.effective()
    }

    pub fn filter_watched(&self) -> bool {
        self.filter_watched.effective()
    }

    pub fn addons(&self) -> Vec<Addon> {
        self.addons.effective()
    }

    pub fn catalogs(&self) -> Vec<crate::media::Media> {
        self.catalogs.effective()
    }

    
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            addons: SettingField {
                default: vec![],
                value: None,
                locked: false,
            },
            catalogs: SettingField {
                default: vec![],
                value: None,
                locked: false,
            },
            version_auto_select: SettingField {
                default: true,
                value: None,
                locked: false,
            },
            filter_watched: SettingField {
                default: false,
                value: None,
                locked: false,
            },
        }
    }
}

impl Settings {
    pub fn update_catalog(mut self, updated: media::Media) -> Self {
        if let Some(ref mut vec) = self.catalogs.value {
            if let Some(existing) = vec.iter_mut().find(|c| c.id == updated.id) {
                *existing = updated;
            }
        }
        self
    }

    pub fn add_catalogs(mut self, other: Vec<media::Media>) -> Self {
        let vec = self.catalogs.value.get_or_insert_with(Vec::new);
        for other_cat in &other {
            if vec.iter().all(|c| c.id != other_cat.id) {
                vec.push(other_cat.clone());
            }
        }
        self
    }

    pub fn into_catalogs(
        &self,
        server_catalogs: Vec<crate::media::Media>,
    ) -> Vec<crate::media::Media> {
        let local = self.catalogs.value.as_ref().unwrap_or(&self.catalogs.default);

        server_catalogs
            .into_iter()
            .map(|col| {
                if let Some(setting) = local.iter().find(|c| c.id == col.id) {
                    crate::media::Media {
                        id: col.id,
                        title: setting.title.clone(),
                        enabled: setting.enabled.clone(),
                        card_variant: setting.card_variant.clone(),
                        ..col
                    }
                } else {
                    col
                }
            })
            .collect()
    }
}

pub fn use_settings() -> Signal<Settings> {
    // use_persistent("settings", || Settings::default())
    use_synced_storage::<LocalStorage, Settings>(SETTINGS_KEY.to_string(), || Settings::default())
}
