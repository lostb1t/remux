use crate::Route;
use crate::{
    components, hooks,
    server::{JellyfinServer, Server, ServerConfig, ServerInstance, ServerKind, StremioServer},
};
use dioxus::prelude::*;
use dioxus::signals::*;
use dioxus_logger::tracing::{debug, error, info};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias as tokio;

#[component]
pub fn LoginView() -> Element {
    let mut server_config = hooks::use_server_config();
    let mut host = use_signal(|| "".to_string());
    let mut username = use_signal(|| "".to_string());
    let mut password = use_signal(|| "".to_string());
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();
    let mut server = hooks::use_server();

    if server_config.read().is_some() {
        debug!("We have a config already, routing to home");
        nav.push(Route::Home {});
    }

    let on_login = move |_| {
        let host = host();
        let username = username();
        let password = password();

        if host.is_empty() || username.is_empty() || password.is_empty() {
            error.set(Some("Please fill in all fields.".to_string()));
            return;
        }

        loading.set(true);
        error.set(None);

        spawn(async move {
            let kind = if host.contains("stremio") {
                ServerKind::Stremio
            } else {
                ServerKind::Jellyfin
            };

            match ServerInstance::from_credentials(
                kind,
                host.clone(),
                username.clone(),
                password.clone(),
            )
            .await
            {
                Ok(server_instance) => {
                    debug!(
                        "Server instance created: {:?}",
                        server_instance.into_config()
                    );
                    server_config.set(Some(server_instance.into_config()));
                    server.set(Some(Arc::new(server_instance)));
                    // spawn(async move {
                    // sleep(Duration::from_millis(5000)).await;
                    // let _ = nav.push(Route::Home {});
                    // signal.set(true);
                    //});
                    // let _ = nav.push(Route::Home {});
                }
                Err(e) => {
                    error.set(Some(format!("Login failed: {}", e)));
                    error!("{e}");
                }
            }

            loading.set(false);
        });
    };

    rsx! {
        div { class: "min-h-screen flex flex-col items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded shadow-md w-full max-w-sm",
                h1 { class: "text-2xl font-bold mb-6 text-center", "Login" }

                input {
                    r#type: "text",
                    placeholder: "Server URL",
                    class: "text-black mb-4 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{host}",
                    name: "host",
                    oninput: move |e| host.set(e.value().clone()),
                }

                input {
                    r#type: "text",
                    placeholder: "Username",
                    class: "text-black mb-4 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{username}",
                    name: "username",
                    oninput: move |e| username.set(e.value().clone()),
                }

                input {
                    r#type: "password",
                    placeholder: "Password",
                    class: "text-black mb-6 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{password}",
                    name: "password",
                    oninput: move |e| password.set(e.value().clone()),
                }

                if let Some(err) = error() {
                    p { class: "text-red-500 text-sm mb-4", "{err}" }
                }

                if loading() {
                    components::Button { disabled: true, "Logging in..." }
                } else {
                    components::Button { onclick: on_login, "Log In" }
                }
            }
        }
    }
}
