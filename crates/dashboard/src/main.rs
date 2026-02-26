use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use shared::sdks::jellyfin::{AuthenticateUserByName, JellyfinAuth};
use shared::sdks::ClientError;
use uuid::Uuid;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

/// Key used by jellyfin-web — shared so both apps see the same session.
const CREDENTIALS_KEY: &str = "jellyfin_credentials";
const DEVICE_ID_KEY: &str = "remux_device_id";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StoredServer {
    id: String,
    name: String,
    manual_address: String,
    access_token: String,
    user_id: String,
    date_last_accessed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct StoredCredentials {
    servers: Vec<StoredServer>,
}

fn get_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

fn get_or_create_device_id() -> String {
    LocalStorage::get::<String>(DEVICE_ID_KEY).unwrap_or_else(|_| {
        let id = Uuid::new_v4().to_string();
        let _ = LocalStorage::set(DEVICE_ID_KEY, &id);
        id
    })
}

fn get_stored_server() -> Option<StoredServer> {
    let creds: StoredCredentials = LocalStorage::get(CREDENTIALS_KEY).ok()?;
    creds.servers.into_iter().next()
}

fn store_credentials(server: StoredServer) {
    let creds = StoredCredentials { servers: vec![server] };
    let _ = LocalStorage::set(CREDENTIALS_KEY, &creds);
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut logged_in = use_signal(|| get_stored_server().is_some());

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        if *logged_in.read() {
            Dashboard { on_logout: move |_| logged_in.set(false) }
        } else {
            Login { on_login: move |_| logged_in.set(true) }
        }
    }
}

#[component]
fn Login(on_login: EventHandler) -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();

        let u = username.peek().clone();
        let p = password.peek().clone();
        let origin = get_origin();
        let device_id = get_or_create_device_id();

        loading.set(true);
        error.set(None);

        spawn(async move {
            let client = match shared::sdks::jellyfin::client(&origin) {
                Ok(c) => c.with_auth(JellyfinAuth::new(&device_id)),
                Err(e) => {
                    error.set(Some(format!("Bad server URL: {e}")));
                    loading.set(false);
                    return;
                }
            };

            let ep = AuthenticateUserByName { username: u, pw: p };
            match client.execute(ep).await {
                Ok(result) => {
                    if let (Some(token), Some(user)) = (result.access_token, result.user) {
                        store_credentials(StoredServer {
                            id: result.server_id,
                            name: "Remux".to_string(),
                            manual_address: origin,
                            access_token: token,
                            user_id: user.id.to_string(),
                            date_last_accessed: 0.0,
                        });
                        on_login.call(());
                    } else {
                        error.set(Some("Login failed: no token in response".into()));
                    }
                }
                Err(ClientError::Unauthorized) => {
                    error.set(Some("Invalid username or password".into()));
                }
                Err(e) => {
                    error.set(Some(format!("Login failed: {e}")));
                }
            }

            loading.set(false);
        });
    };

    rsx! {
        div {
            class: "min-h-screen flex items-center justify-center bg-gray-900",
            div {
                class: "bg-gray-800 p-8 rounded-lg shadow-lg w-full max-w-md",
                h1 { class: "text-2xl font-bold text-white mb-6 text-center", "Remux Admin" }

                if let Some(err) = error.read().as_ref() {
                    div {
                        class: "mb-4 p-3 bg-red-900 text-red-200 rounded text-sm",
                        "{err}"
                    }
                }

                form {
                    onsubmit: on_submit,
                    div { class: "mb-4",
                        label { class: "block text-gray-300 text-sm mb-1", r#for: "username", "Username" }
                        input {
                            id: "username",
                            r#type: "text",
                            class: "w-full px-3 py-2 bg-gray-700 text-white rounded border border-gray-600 focus:border-blue-500 focus:outline-none",
                            value: "{username}",
                            oninput: move |e| username.set(e.value()),
                            required: true,
                            autocomplete: "username",
                        }
                    }
                    div { class: "mb-6",
                        label { class: "block text-gray-300 text-sm mb-1", r#for: "password", "Password" }
                        input {
                            id: "password",
                            r#type: "password",
                            class: "w-full px-3 py-2 bg-gray-700 text-white rounded border border-gray-600 focus:border-blue-500 focus:outline-none",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                            autocomplete: "current-password",
                        }
                    }
                    button {
                        r#type: "submit",
                        class: "w-full py-2 px-4 bg-blue-600 hover:bg-blue-700 text-white font-medium rounded disabled:opacity-50 transition-colors",
                        disabled: *loading.read(),
                        if *loading.read() { "Signing in…" } else { "Sign In" }
                    }
                }
            }
        }
    }
}

#[component]
fn Dashboard(on_logout: EventHandler) -> Element {
    rsx! {
        div {
            class: "min-h-screen bg-gray-900 text-white p-8",
            div {
                class: "max-w-4xl mx-auto",
                div {
                    class: "flex justify-between items-center mb-8",
                    h1 { class: "text-3xl font-bold", "Remux Admin" }
                    button {
                        class: "px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors",
                        onclick: move |_| {
                            LocalStorage::delete(CREDENTIALS_KEY);
                            on_logout.call(());
                        },
                        "Sign Out"
                    }
                }
                p { class: "text-gray-400", "Dashboard under construction." }
            }
        }
    }
}
