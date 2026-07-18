use crate::{
    components::{NavIcon, ThemeModeSegment},
    router::Route,
    state::{get_stored_server, AppState, CREDENTIALS_KEY},
};
use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use remux_sdks::remux::PublicSystemInfo;

#[component]
pub(crate) fn NavItem(
    label: &'static str,
    icon: &'static str,
    active: bool,
    on_click: EventHandler,
) -> Element {
    rsx! {
        button {
            class: if active { "nav-item nav-item-active" } else { "nav-item" },
            onclick: move |_| on_click.call(()),
            NavIcon { name: icon }
            span { class: "nav-label", "{label}" }
        }
    }
}

#[component]
pub(crate) fn NavSubItem(
    label: &'static str,
    icon: &'static str,
    active: bool,
    on_click: EventHandler,
) -> Element {
    rsx! {
        button {
            class: if active { "nav-sub-item nav-sub-item-active" } else { "nav-sub-item" },
            onclick: move |_| on_click.call(()),
            NavIcon { name: icon }
            span { class: "nav-label", "{label}" }
        }
    }
}

#[component]
pub(crate) fn SidebarGroup(
    label: &'static str,
    icon: &'static str,
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
                span { class: "nav-group-title",
                    NavIcon { name: icon }
                    span { "{label}" }
                }
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

    // The server's own name drives the brand + breadcrumb root, so the panel
    // identifies the server it manages rather than the generic product name.
    // Seed from the stored name, then refresh from the live server info (the
    // stored value can be a stale placeholder from an older login).
    let mut server_name = use_signal(|| {
        let n = app_state
            .server
            .name
            .trim()
            .to_string();
        if n.is_empty() {
            "Remux".to_string()
        } else {
            n
        }
    });
    {
        let client = app_state
            .client
            .clone();
        use_effect(move || {
            let client = client.clone();
            spawn(async move {
                if let Ok(info) = client
                    .execute(PublicSystemInfo::default())
                    .await
                {
                    let name = info
                        .server_name
                        .trim()
                        .to_string();
                    if !name.is_empty() {
                        server_name.set(name);
                    }
                }
            });
        });
    }

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
        Route::AccessDevicesRoute => "Devices",
        Route::TasksRoute => "Tasks",
        Route::SystemLogsRoute => "Logs",
        Route::SystemActivityRoute => "Activity",
        Route::SystemTelemetryRoute => "Telemetry",
        Route::SessionsRoute => "Sessions",
        Route::NotFound { .. } => "",
    };

    // Breadcrumb parent section + a backlink to that section's landing page, so
    // users always know where they are and can jump back up one level.
    let section: Option<(&str, Route)> = match route {
        Route::LibraryRoute | Route::IptvRoute => {
            Some(("Content", Route::LibraryRoute))
        }
        Route::StreamingGroupsRoute
        | Route::StreamingProbingRoute
        | Route::StreamingP2pRoute => Some(("Streaming", Route::StreamingGroupsRoute)),
        Route::SettingsGeneralRoute
        | Route::SettingsPlaybackRoute
        | Route::SettingsSearchRoute
        | Route::SettingsJellyfinSyncRoute
        | Route::SettingsBrandingRoute
        | Route::SettingsAppearanceRoute
        | Route::SettingsIntroRoute => Some(("Settings", Route::SettingsGeneralRoute)),
        Route::AccessUsersRoute
        | Route::AccessApiKeysRoute
        | Route::AccessDevicesRoute => Some(("Access", Route::AccessUsersRoute)),
        Route::SystemLogsRoute
        | Route::SystemActivityRoute
        | Route::SystemTelemetryRoute => Some(("System", Route::SystemActivityRoute)),
        _ => None,
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
                    span { class: "brand-eyebrow", "Admin" }
                    h1 { class: "brand-title", style: "margin:0", "{server_name}" }
                }

                div { class: "sidebar-nav",
                    NavItem {
                        label: "Dashboard",
                        icon: "dashboard",
                        active: route == Route::DashboardRoute,
                        on_click: move |_| { navigator().push(Route::DashboardRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Addons",
                        icon: "addons",
                        active: route == Route::AddonsRoute,
                        on_click: move |_| { navigator().push(Route::AddonsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Tasks",
                        icon: "tasks",
                        active: route == Route::TasksRoute,
                        on_click: move |_| { navigator().push(Route::TasksRoute); sidebar_open.set(false); },
                    }

                    div { class: "nav-divider" }

                    SidebarGroup {
                        label: "Content",
                        icon: "content",
                        active: matches!(route, Route::LibraryRoute | Route::IptvRoute),
                        NavSubItem {
                            label: "Library",
                            icon: "library",
                            active: route == Route::LibraryRoute,
                            on_click: move |_| { navigator().push(Route::LibraryRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "IPTV",
                            icon: "iptv",
                            active: route == Route::IptvRoute,
                            on_click: move |_| { navigator().push(Route::IptvRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Streaming",
                        icon: "streaming",
                        active: matches!(route, Route::StreamingGroupsRoute | Route::StreamingProbingRoute | Route::StreamingP2pRoute),
                        NavSubItem {
                            label: "Groups",
                            icon: "groups",
                            active: route == Route::StreamingGroupsRoute,
                            on_click: move |_| { navigator().push(Route::StreamingGroupsRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Probing",
                            icon: "probing",
                            active: route == Route::StreamingProbingRoute,
                            on_click: move |_| { navigator().push(Route::StreamingProbingRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "P2P",
                            icon: "p2p",
                            active: route == Route::StreamingP2pRoute,
                            on_click: move |_| { navigator().push(Route::StreamingP2pRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Settings",
                        icon: "settings",
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
                            icon: "general",
                            active: route == Route::SettingsGeneralRoute,
                            on_click: move |_| { navigator().push(Route::SettingsGeneralRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Playback",
                            icon: "playback",
                            active: route == Route::SettingsPlaybackRoute,
                            on_click: move |_| { navigator().push(Route::SettingsPlaybackRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Search",
                            icon: "search",
                            active: route == Route::SettingsSearchRoute,
                            on_click: move |_| { navigator().push(Route::SettingsSearchRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Jellyfin Sync",
                            icon: "sync",
                            active: route == Route::SettingsJellyfinSyncRoute,
                            on_click: move |_| { navigator().push(Route::SettingsJellyfinSyncRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Intro",
                            icon: "intro",
                            active: route == Route::SettingsIntroRoute,
                            on_click: move |_| { navigator().push(Route::SettingsIntroRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Branding",
                            icon: "branding",
                            active: route == Route::SettingsBrandingRoute,
                            on_click: move |_| { navigator().push(Route::SettingsBrandingRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Appearance",
                            icon: "appearance",
                            active: route == Route::SettingsAppearanceRoute,
                            on_click: move |_| { navigator().push(Route::SettingsAppearanceRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "Access",
                        icon: "access",
                        active: matches!(route, Route::AccessUsersRoute | Route::AccessApiKeysRoute | Route::AccessDevicesRoute),
                        NavSubItem {
                            label: "Users",
                            icon: "users",
                            active: route == Route::AccessUsersRoute,
                            on_click: move |_| { navigator().push(Route::AccessUsersRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "API Keys",
                            icon: "api-keys",
                            active: route == Route::AccessApiKeysRoute,
                            on_click: move |_| { navigator().push(Route::AccessApiKeysRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Devices",
                            icon: "devices",
                            active: route == Route::AccessDevicesRoute,
                            on_click: move |_| { navigator().push(Route::AccessDevicesRoute); sidebar_open.set(false); },
                        }
                    }

                    SidebarGroup {
                        label: "System",
                        icon: "system",
                        active: matches!(route, Route::SystemLogsRoute | Route::SystemActivityRoute),
                        NavSubItem {
                            label: "Activity",
                            icon: "activity",
                            active: route == Route::SystemActivityRoute,
                            on_click: move |_| { navigator().push(Route::SystemActivityRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Logs",
                            icon: "logs",
                            active: route == Route::SystemLogsRoute,
                            on_click: move |_| { navigator().push(Route::SystemLogsRoute); sidebar_open.set(false); },
                        }
                        NavSubItem {
                            label: "Telemetry",
                            icon: "activity",
                            active: route == Route::SystemTelemetryRoute,
                            on_click: move |_| { navigator().push(Route::SystemTelemetryRoute); sidebar_open.set(false); },
                        }
                    }

                    div { class: "nav-divider" }

                    NavItem {
                        label: "Sessions",
                        icon: "sessions",
                        active: route == Route::SessionsRoute,
                        on_click: move |_| { navigator().push(Route::SessionsRoute); sidebar_open.set(false); },
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
                        aria_label: "Toggle navigation",
                        onclick: move |_| {
                            let open = !*sidebar_open.read();
                            sidebar_open.set(open);
                        },
                        "☰"
                    }
                    nav { class: "breadcrumb", aria_label: "Breadcrumb",
                        // Home root — always a one-click path back to the Dashboard,
                        // except when we're already there.
                        if route != Route::DashboardRoute {
                            button {
                                class: "breadcrumb-link breadcrumb-home",
                                title: "Dashboard",
                                onclick: move |_| { navigator().push(Route::DashboardRoute); },
                                "Home"
                            }
                            span { class: "breadcrumb-sep", "›" }
                        }
                        if let Some((label, target)) = section.clone() {
                            button {
                                class: "breadcrumb-link",
                                onclick: move |_| { navigator().push(target.clone()); },
                                "{label}"
                            }
                            span { class: "breadcrumb-sep", "›" }
                        }
                        h1 { class: "main-title", "{page_title}" }
                    }
                }

                div { class: "shell",
                    Outlet::<Route> {}
                }
            }
        }
    }
}
