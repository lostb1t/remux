use crate::{
    components::ThemeModeSegment,
    router::Route,
    state::{get_stored_server, AppState, CREDENTIALS_KEY},
};
use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};

#[component]
pub(crate) fn NavItem(
    label: &'static str,
    active: bool,
    on_click: EventHandler,
) -> Element {
    rsx! {
        button {
            class: if active { "nav-item nav-item-active" } else { "nav-item" },
            onclick: move |_| on_click.call(()),
            "{label}"
        }
    }
}

#[component]
pub(crate) fn NavSubItem(
    label: &'static str,
    active: bool,
    on_click: EventHandler,
) -> Element {
    rsx! {
        button {
            class: if active { "nav-sub-item nav-sub-item-active" } else { "nav-sub-item" },
            onclick: move |_| on_click.call(()),
            "{label}"
        }
    }
}

#[component]
pub(crate) fn SidebarGroup(
    label: &'static str,
    active: bool,
    children: Element,
) -> Element {
    let storage_key = format!("sidebar_group_{label}");
    let mut open =
        use_signal(|| LocalStorage::get::<bool>(&storage_key).unwrap_or(true));
    let expanded = *open.read() || active;
    rsx! {
        div { class: "nav-group",
            button {
                class: if active { "nav-group-header nav-group-header-active" } else { "nav-group-header" },
                onclick: move |_| {
                    let v = !*open.read();
                    open.set(v);
                    let _ = LocalStorage::set(&storage_key, v);
                },
                span { "{label}" }
                span { class: "nav-group-chevron", if expanded { "▾" } else { "▸" } }
            }
            if expanded {
                div { class: "nav-group-items", {children} }
            }
        }
    }
}

#[component]
pub fn DashboardLayout() -> Element {
    let server = match get_stored_server() {
        Some(s) => s,
        None => return rsx! { div { "Not logged in" } },
    };

    let app_state = AppState::new(server);
    use_context_provider(|| app_state.clone());

    let mut logged_in = use_context::<Signal<bool>>();
    let mut sidebar_open = use_signal(|| false);
    let route = use_route::<Route>();

    let page_title = match route {
        Route::DashboardRoute => "Remux",
        Route::AddonsRoute => "Addons",
        Route::LibraryRoute => "Library",
        Route::IptvRoute => "IPTV",
        Route::StreamingGroupsRoute => "Stream Groups",
        Route::StreamingProbingRoute => "Probing",
        Route::StreamingP2pRoute => "P2P",
        Route::SettingsGeneralRoute => "General",
        Route::SettingsPlaybackRoute => "Playback",
        Route::SettingsSearchRoute => "Search",
        Route::SettingsJellyfinSyncRoute => "Jellyfin Sync",
        Route::SettingsBrandingRoute => "Branding",
        Route::SettingsAppearanceRoute => "Appearance",
        Route::SettingsIntroRoute => "Intro",
        Route::AccessUsersRoute => "Users",
        Route::AccessApiKeysRoute => "API Keys",
        Route::TasksRoute => "Tasks",
        Route::ActivityRoute => "Activity",
        Route::NotFound { .. } => "",
    };

    rsx! {
        div { class: "layout",
            if *sidebar_open.read() {
                div {
                    class: "sidebar-overlay",
                    onclick: move |_| sidebar_open.set(false),
                }
            }

            nav {
                class: if *sidebar_open.read() { "sidebar sidebar-open" } else { "sidebar" },

                div { class: "sidebar-brand",
                    h1 { class: "brand-title", style: "margin:0", "Remux" }
                }

                div { class: "sidebar-nav",
                    NavItem {
                        label: "Dashboard",
                        active: route == Route::DashboardRoute,
                        on_click: move |_| { navigator().push(Route::DashboardRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Addons",
                        active: route == Route::AddonsRoute,
                        on_click: move |_| { navigator().push(Route::AddonsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Tasks",
                        active: route == Route::TasksRoute,
                        on_click: move |_| { navigator().push(Route::TasksRoute); sidebar_open.set(false); },
                    }

                    div { class: "nav-divider" }

                    SidebarGroup {
                        label: "Content",
                        active: matches!(route, Route::LibraryRoute | Route::IptvRoute),
                        NavSubItem {
                            label: "Library",
                            active: route == Route::LibraryRoute,
                            on_click: move |_| { navigator().push(Route::LibraryRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "IPTV",
                            active: route == Route::IptvRoute,
                            on_click: move |_| { navigator().push(Route::IptvRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Streaming",
                        active: matches!(route, Route::StreamingGroupsRoute | Route::StreamingProbingRoute | Route::StreamingP2pRoute),
                        NavSubItem {
                            label: "Groups",
                            active: route == Route::StreamingGroupsRoute,
                            on_click: move |_| { navigator().push(Route::StreamingGroupsRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Probing",
                            active: route == Route::StreamingProbingRoute,
                            on_click: move |_| { navigator().push(Route::StreamingProbingRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "P2P",
                            active: route == Route::StreamingP2pRoute,
                            on_click: move |_| { navigator().push(Route::StreamingP2pRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Settings",
                        active: matches!(route,
                            Route::SettingsGeneralRoute
                            | Route::SettingsPlaybackRoute
                            | Route::SettingsSearchRoute
                            | Route::SettingsJellyfinSyncRoute
                            | Route::SettingsBrandingRoute
                            | Route::SettingsAppearanceRoute
                            | Route::SettingsIntroRoute
                        ),
                        NavSubItem {
                            label: "General",
                            active: route == Route::SettingsGeneralRoute,
                            on_click: move |_| { navigator().push(Route::SettingsGeneralRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Playback",
                            active: route == Route::SettingsPlaybackRoute,
                            on_click: move |_| { navigator().push(Route::SettingsPlaybackRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Search",
                            active: route == Route::SettingsSearchRoute,
                            on_click: move |_| { navigator().push(Route::SettingsSearchRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Jellyfin Sync",
                            active: route == Route::SettingsJellyfinSyncRoute,
                            on_click: move |_| { navigator().push(Route::SettingsJellyfinSyncRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Intro",
                            active: route == Route::SettingsIntroRoute,
                            on_click: move |_| { navigator().push(Route::SettingsIntroRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Branding",
                            active: route == Route::SettingsBrandingRoute,
                            on_click: move |_| { navigator().push(Route::SettingsBrandingRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Appearance",
                            active: route == Route::SettingsAppearanceRoute,
                            on_click: move |_| { navigator().push(Route::SettingsAppearanceRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Access",
                        active: matches!(route, Route::AccessUsersRoute | Route::AccessApiKeysRoute),
                        NavSubItem {
                            label: "Users",
                            active: route == Route::AccessUsersRoute,
                            on_click: move |_| { navigator().push(Route::AccessUsersRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "API Keys",
                            active: route == Route::AccessApiKeysRoute,
                            on_click: move |_| { navigator().push(Route::AccessApiKeysRoute); sidebar_open.set(false); },
                        }
                    }

                    div { class: "nav-divider" }

                    NavItem {
                        label: "Activity",
                        active: route == Route::ActivityRoute,
                        on_click: move |_| { navigator().push(Route::ActivityRoute); sidebar_open.set(false); },
                    }
                }

                div { class: "sidebar-footer",
                    // Quick theme toggle — always visible; mirrors the Appearance page.
                    ThemeModeSegment {}
                    a {
                        class: "btn btn-ghost",
                        style: "width:100%;margin-bottom:8px",
                        href: "/",
                        "Jellyfin Web"
                    }
                    button {
                        class: "btn btn-ghost",
                        style: "width:100%",
                        onclick: move |_| {
                            LocalStorage::delete(CREDENTIALS_KEY);
                            logged_in.set(false);
                        },
                        "Sign Out"
                    }
                }
            }

            div { class: "main",
                div { class: "main-header",
                    button {
                        class: "hamburger",
                        onclick: move |_| {
                            let open = !*sidebar_open.read();
                            sidebar_open.set(open);
                        },
                        "☰"
                    }
                    h2 { class: "main-title", "{page_title}" }
                }

                div { class: "shell",
                    Outlet::<Route> {}
                }
            }
        }
    }
}
