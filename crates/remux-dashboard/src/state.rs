use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use remux_sdks::{remux::JellyfinAuth, RestClient};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
pub const THEME_CSS: Asset = asset!("/assets/theme.css");

pub const CREDENTIALS_KEY: &str = "remux_credentials";
pub const DEVICE_ID_KEY: &str = "remux_device_id";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StoredServer {
    pub id: String,
    pub name: String,
    pub manual_address: String,
    pub access_token: String,
    pub user_id: String,
    pub date_last_accessed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StoredCredentials {
    pub servers: Vec<StoredServer>,
}

#[derive(Clone)]
pub struct AppState {
    pub server: StoredServer,
    pub client: RestClient<JellyfinAuth>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("server", &self.server)
            .field("client", &"<RestClient>")
            .finish()
    }
}

impl PartialEq for AppState {
    fn eq(&self, other: &Self) -> bool {
        self.server
            .id
            == other
                .server
                .id
    }
}

impl AppState {
    pub fn new(server: StoredServer) -> Self {
        let device_id = get_or_create_device_id();
        let auth = JellyfinAuth::new(&device_id).with_token(
            server
                .access_token
                .clone(),
        );
        let client = remux_sdks::remux::client(&server.manual_address)
            .unwrap_or_else(|_| panic!("invalid server url: {}", server.manual_address))
            .with_auth(auth);
        Self { server, client }
    }
}

pub fn detect_image_content_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => {
            "image/webp"
        }
        _ => "image/jpeg",
    }
}

/// Extracts HH:MM from a DateTime Display string ("2026-02-26 18:30:38 UTC").
pub fn fmt_time(dt: impl std::fmt::Display) -> String {
    let s = dt.to_string();
    s.chars()
        .skip(11)
        .take(5)
        .collect()
}

pub fn get_origin() -> String {
    web_sys::window()
        .and_then(|w| {
            w.location()
                .origin()
                .ok()
        })
        .unwrap_or_default()
}

pub fn browser_metadata_country_code() -> String {
    web_sys::window()
        .and_then(|w| {
            w.navigator()
                .language()
        })
        .and_then(|language| {
            language
                .split(['-', '_'])
                .skip(1)
                .filter(|part| {
                    part.len() == 2
                        && part
                            .chars()
                            .all(|c| c.is_ascii_alphabetic())
                })
                .last()
                .map(|part| part.to_ascii_uppercase())
        })
        .unwrap_or_else(|| "US".to_string())
}

pub fn get_or_create_device_id() -> String {
    LocalStorage::get::<String>(DEVICE_ID_KEY).unwrap_or_else(|_| {
        let id = Uuid::new_v4().to_string();
        let _ = LocalStorage::set(DEVICE_ID_KEY, &id);
        id
    })
}

pub fn get_stored_server() -> Option<StoredServer> {
    let creds: StoredCredentials = LocalStorage::get(CREDENTIALS_KEY).ok()?;
    creds
        .servers
        .into_iter()
        .next()
}

pub fn store_credentials(server: StoredServer) {
    let _ = LocalStorage::set(
        CREDENTIALS_KEY,
        &StoredCredentials {
            servers: vec![server],
        },
    );
}
