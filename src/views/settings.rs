use crate::components;
use crate::hooks;
use crate::server;
use crate::settings::{use_settings, Addon};
use crate::Route;
use dioxus::prelude::*;
//use dioxus::web::WebEventExt;
use crate::error;
use anyhow::anyhow;
use dioxus_logger::tracing::{debug, info};
use dioxus_primitives::switch::{Switch, SwitchThumb};
use std::str::FromStr;
use strum::IntoEnumIterator;
use tracing_subscriber::field::debug;

#[component]
pub fn Settings() -> Element {
    let mut server = hooks::use_server();
    let mut config = hooks::use_server_config();
    let nav = use_navigator();
    let mut settings = use_settings();

    //return Err(error::AppError::Other("yoooo".to_string()))?;

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
                    }
                }
                li {
                    "Filter watched"
                    components::Switch {
                        enabled: settings.read().filter_watched(),
                        on_toggle: move |new_state| {
                            let mut s = settings();
                            s.filter_watched.set(!settings.read().filter_watched());
                            settings.set(s);
                        },
                    }
                }
                li {
                    a {
                        //   icon: "👤",
                        onclick: {
                            move |_| {
                                config.set(None);
                                server.set(None);
                            }
                        },
                        "Logout"
                    }
                }
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
    let server = hooks::use_server()().unwrap();
    let mut new_catalog = use_signal(|| String::new());

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
            );
        }
    };

    if catalogs != settings().catalogs {
        settings.write().catalogs = catalogs.clone();
    }

    //debug!(?catalogs, "Catalogs loaded");

    rsx! {
        div { class: "space-y-4 p-4",
            h1 { class: "text-2xl font-bold", "Manage Catalogs" }

            ul { class: "space-y-2",
                for (i , catalog) in catalogs.effective().iter().enumerate() {
                    {

                        //debug!(?updated, "uoho");
                        //debug!(?s.catalogs, "uoho");

                        let catalog = catalog.clone();
                        rsx! {
                            li {
                                key: "{i}",
                                class: "flex items-center justify-between px-4 py-3 bg-zinc-900 rounded-lg hover:bg-zinc-800 transition space-x-2",



                                //let mut settings = settings.read().clone();
                                // debug!(?current, "Toggling catalog");

                                //}

                                span { class: "text-white font-medium", "{catalog.title}" }

                                div { class: "flex items-center space-x-2",

                                    select {
                                        class: "bg-gray-800 text-white px-3 py-2 rounded",
                                        onchange: move |evt| {
                                            let mut updated = catalog.clone();
                                            let id = evt.value().clone();
                                            updated.card_variant.set(components::CardVariant::from_str(&id).unwrap());
                                            let mut s = settings.read().clone();
                                            s = s.update_catalog(updated);
                                            settings.set(s);
                                        },
                                        for card in components::CardVariant::iter() {
                                            option {


                                                value: "{card}",
                                                selected: catalog.card_variant.effective() == card,
                                                "{card}"
                                            }
                                        }
                                    }
                                    button {
                                        onclick: move |_| {
                                            let mut s = settings.read().clone();

                                            let vec = s.catalogs.value.get_or_insert_with(|| s.catalogs.default.clone());
                                            if i > 0 {
                                                vec.swap(i, i - 1);
                                                settings.set(s);
                                            }
                                        },
                                        class: "text-sm px-2 py-1 bg-zinc-700 rounded hover:bg-zinc-600",
                                        "↑"
                                    }
                                    button {
                                        onclick: move |_| {
                                            let mut s = settings.read().clone();

                                            let vec = s.catalogs.value.get_or_insert_with(|| s.catalogs.default.clone());
                                            if i + 1 < vec.len() {
                                                vec.swap(i, i + 1);
                                                settings.set(s);
                                            }
                                        },
                                        class: "text-sm px-2 py-1 bg-zinc-700 rounded hover:bg-zinc-600",
                                        "↓"
                                    }
                                    Switch {
                                        class: {
                                            if catalog.enabled.effective() {
                                                "relative inline-flex h-6 w-11 items-center rounded-full bg-green-600"
                                            } else {
                                                "relative inline-flex h-6 w-11 items-center rounded-full bg-zinc-700"
                                            }
                                        },
                                        checked: catalog.enabled.effective(),
                                        on_checked_change: {
                                            let catalog = catalog.clone();
                                            move |new_state| {
                                                let mut catalog = catalog.clone();

                                                debug!(? new_state, "Toggling catalog");
                                                catalog.enabled.set(new_state);
                                                let mut s = settings.read().clone();
                                                s = s.update_catalog(catalog);
                                                settings.set(s);
                                            }
                                        },
                                        aria_label: "Toggle catalog",
                                        SwitchThumb {
                                            class: {
                                                if catalog.enabled.effective() {
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
    }
}
