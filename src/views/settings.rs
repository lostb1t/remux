use crate::components;
use crate::hooks;
use crate::server;
use crate::settings::{use_settings, Addon};
use crate::Route;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_logger::tracing::{debug, info};
use dioxus_primitives::switch::{Switch, SwitchThumb};
use tracing_subscriber::field::debug;

#[component]
pub fn Settings() -> Element {
    let mut server = hooks::use_server();
    let mut config = hooks::use_server_config();
    let nav = use_navigator();
    let mut settings = use_settings();

    rsx! {
        div {
            h1 { "Settings" }
            ul {
           //   title: "Content",
           li {
                SettingRow {
                    icon: "👤",
                    label: "Catalogs",
                    to: Route::SettingsCatalogView {},
                }}
                li {
                  "Filter watched"
                components::Switch {
    enabled: settings.read().filter_watched,
    on_toggle: move |new_state| {
      let mut s = settings();
      s.filter_watched = !settings.read().filter_watched;
      settings.set(s);
    }
}}
li {
                a {
                    //   icon: "👤",
                    onclick: {
                        move |_| {
                            server.set(None);
                            config.set(None);
                            nav.push(Route::LoginView {});
                        }
                    },
                    "Logout"
                                // to: Route::SettingsCatalogqView {},
                }
              }
            }
        }
    }
}

#[component]
pub fn SettingsAddonsView() -> Element {
    let mut settings = use_settings();
    let mut new_addon = use_signal(|| String::new());

    rsx! {
        div {
            h1 { "Manage Addons" }

            ul {
                for (i , addon) in settings().addons.iter().enumerate() {
                    li {
                        "{addon.name} — "

                        button {
                            onclick: move |_| {
                                let mut s = settings.write();
                                if let Some(a) = s.addons.get_mut(i) {
                                    a.enabled = !a.enabled;
                                }
                            },
                        }

                        button {
                            onclick: move |_| {
                                let mut s = settings.write();
                                s.addons.remove(i);
                            },
                            "Delete"
                        }
                    }
                }
            }

            form {
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    if !new_addon().trim().is_empty() {
                        settings
                            .write()
                            .addons
                            .push(Addon {
                                name: new_addon().trim().to_string(),
                                enabled: true,
                            });
                        new_addon.set(String::new());
                    }
                },
                input {
                    value: "{new_addon()}",
                    oninput: move |e| new_addon.set(e.value().clone()),
                    placeholder: "New addon name",
                }
                components::Button { "Add" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SettingRowProps {
    pub icon: &'static str,
    pub label: &'static str,
    pub to: Route,
}

#[component]
pub fn SettingRow(props: SettingRowProps) -> Element {
    let content = rsx!(
        div { class: "flex items-center justify-between px-4 py-3 bg-zinc-900 rounded-lg hover:bg-zinc-800 transition",
            div { class: "flex items-center space-x-3",
                span { class: "text-xl", "{props.icon}" }
                span { class: "text-white font-medium", "{props.label}" }
            }
        }
    );

    rsx!(
        Link { to: props.to, class: "block", {content} }
    )
}

#[derive(Props, Clone, PartialEq)]
pub struct SettingsSectionProps {
    pub title: &'static str,
    children: Element,
}

#[component]
pub fn SettingsSection(props: SettingsSectionProps) -> Element {
    rsx! {
        div { class: "space-y-2",
            span { class: "text-xs text-zinc-400 uppercase tracking-widest px-1", "{props.title}" }
            div { class: "space-y-1", {props.children} }
        }
    }
}

#[component]
pub fn SettingsCatalogView() -> Element {
    let mut settings = use_settings();
    let server = hooks::consume_server().expect("missing server");
    let mut new_catalog = use_signal(|| String::new());

    // debug!("SettingsCatalogView: {:?}", settings());

    let server_catalogs = use_resource(move || {
        let server = server.clone();
        async move { server.get_catalogs().await }
    })
    .suspend()?;

    let server_catalogs = server_catalogs.read();

    let catalogs = match server_catalogs.as_ref() {
        Ok(data) => settings().add_catalogs(data.clone()).catalogs,
        Err(_) => {
            return rsx!(
                div { "Failed to load catalogs" }
            )
        }
    };

    if catalogs != settings().catalogs {
        settings.write().catalogs = catalogs.clone();
    };

    rsx! {
        div { class: "space-y-4 p-4",
            h1 { class: "text-2xl font-bold", "Manage Catalogs" }

            ul { class: "space-y-2",
                for (i , catalog) in catalogs.iter().enumerate() {
                    li {
                        key: "{i}",
                        class: "flex items-center justify-between px-4 py-3 bg-zinc-900 rounded-lg hover:bg-zinc-800 transition space-x-2",

                        span { class: "text-white font-medium", "{catalog.title}" }

                        div { class: "flex items-center space-x-2",
                            button {
                                onclick: move |_| {
                                    let mut s = settings.read().clone();
                                    if i > 0 {
                                        s.catalogs.swap(i, i - 1);
                                        settings.set(s);
                                    }
                                },
                                class: "text-sm px-2 py-1 bg-zinc-700 rounded hover:bg-zinc-600",
                                "↑"
                            }
                            button {
                                onclick: move |_| {
                                    let mut s = settings.read().clone();
                                    if i + 1 < s.catalogs.len() {
                                        s.catalogs.swap(i, i + 1);
                                        settings.set(s);
                                    }
                                },
                                class: "text-sm px-2 py-1 bg-zinc-700 rounded hover:bg-zinc-600",
                                "↓"
                            }

                            Switch {
                                class: {
                                    if catalog.enabled {
                                        "relative inline-flex h-6 w-11 items-center rounded-full bg-green-600"
                                    } else {
                                        "relative inline-flex h-6 w-11 items-center rounded-full bg-zinc-700"
                                    }
                                },
                                checked: catalog.enabled,
                                on_checked_change: {
                                    let catalog = catalog.clone();
                                    move |new_state| {
                                        let mut current = settings.read().clone();
                                        if let Some(existing) = current
                                            .catalogs
                                            .iter_mut()
                                            .find(|c| c.id == catalog.id)
                                        {
                                            existing.enabled = new_state;
                                            settings.set(current);
                                        }
                                    }
                                },
                                aria_label: "Toggle catalog",
                                SwitchThumb {
                                    class: {
                                        if catalog.enabled {
                                            "inline-block h-4 w-4 transform rounded-full bg-white transition translate-x-6"
                                        } else {
                                            "inline-block h-4 w-4 transform rounded-full bg-white transition translate-x-1"
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
