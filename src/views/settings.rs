use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use crate::settings::{use_settings, Addon};
use crate::Route;
use crate::components;


#[component]
pub fn Settings() -> Element {
    rsx! {
        div {
            h1 { "Settings" }
            ul {
                li {
                    Link { class: "p-3", to: Route::SettingsAddonsView {}, "Addons" }
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
                for (i, addon) in settings().addons.iter().enumerate() {
                    li {
                        "{addon.name} — "
                        //strong { "{if addon.enabled { "Enabled" } else { "Disabled" }}" }

                        button {
                            onclick: move |_| {
                                let mut s = settings.write();
                                if let Some(a) = s.addons.get_mut(i) {
                                    a.enabled = !a.enabled;
                                }
                            },
                          //  "{if addon.enabled { "Disable" } else { "Enable" }}"
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
                        settings.write().addons.push(Addon {
                            name: new_addon().trim().to_string(),
                            enabled: true,
                        });
                        new_addon.set(String::new());
                    }
                },
                input {
                    value: "{new_addon()}",
                    oninput: move |e| new_addon.set(e.value().clone()),
                    placeholder: "New addon name"
                }
                components::Button { variant: "primary", "Add" }
            }
        }
    }
}