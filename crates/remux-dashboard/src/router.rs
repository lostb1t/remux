use crate::{
    components::{SessionsCard, TasksCard},
    layout::DashboardLayout,
    pages::*,
    state::AppState,
};
use dioxus::prelude::*;

#[derive(Clone, Routable, PartialEq, Debug)]
pub enum Route {
    #[layout(DashboardLayout)]
    #[route("/")]
    DashboardRoute,
    #[route("/addons")]
    AddonsRoute,
    #[route("/content/library")]
    LibraryRoute,
    #[route("/content/iptv")]
    IptvRoute,
    #[route("/streaming/groups")]
    StreamingGroupsRoute,
    #[route("/streaming/probing")]
    StreamingProbingRoute,
    #[route("/streaming/p2p")]
    StreamingP2pRoute,
    #[route("/settings/general")]
    SettingsGeneralRoute,
    #[route("/settings/playback")]
    SettingsPlaybackRoute,
    #[route("/settings/search")]
    SettingsSearchRoute,
    #[route("/settings/jellyfin-sync")]
    SettingsJellyfinSyncRoute,
    #[route("/settings/intro")]
    SettingsIntroRoute,
    #[route("/settings/branding")]
    SettingsBrandingRoute,
    #[route("/settings/appearance")]
    SettingsAppearanceRoute,
    #[route("/access/users")]
    AccessUsersRoute,
    #[route("/access/apikeys")]
    AccessApiKeysRoute,
    #[route("/access/devices")]
    AccessDevicesRoute,
    #[route("/tasks")]
    TasksRoute,
    #[route("/system/logs")]
    SystemLogsRoute,
    #[route("/system/activity")]
    SystemActivityRoute,
    #[route("/system/telemetry")]
    SystemTelemetryRoute,
    #[route("/sessions")]
    SessionsRoute,
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
pub(crate) fn DashboardRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { DashboardPage { app_state } }
}

#[component]
pub(crate) fn AddonsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { AddonsPage { app_state } }
}

#[component]
pub(crate) fn LibraryRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { CollectionsPage { app_state } }
}

#[component]
pub(crate) fn IptvRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { IptvPage { app_state } }
}

#[component]
pub(crate) fn StreamingGroupsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { StreamGroupsCard { app_state } }
}

#[component]
pub(crate) fn StreamingProbingRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ProbeSettingsCard { app_state } }
}

#[component]
pub(crate) fn StreamingP2pRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { P2pSettingsCard { app_state } }
}

#[component]
pub(crate) fn SettingsGeneralRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ServerSettingsCard { app_state } }
}

#[component]
pub(crate) fn SettingsPlaybackRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { PlaybackSettingsCard { app_state } }
}

#[component]
pub(crate) fn SettingsSearchRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SearchSettingsCard { app_state } }
}

#[component]
pub(crate) fn SettingsJellyfinSyncRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { JellyfinImportCard { app_state } }
}

#[component]
pub(crate) fn SettingsIntroRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { IntroSettingsCard { app_state } }
}

#[component]
pub(crate) fn SettingsBrandingRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { BrandingPage { app_state } }
}

#[component]
pub(crate) fn SettingsAppearanceRoute() -> Element {
    rsx! { AppearancePage {} }
}

#[component]
pub(crate) fn AccessUsersRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { UsersPage { app_state } }
}

#[component]
pub(crate) fn AccessApiKeysRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ApiKeysPage { app_state } }
}

#[component]
pub(crate) fn AccessDevicesRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { DevicesPage { app_state } }
}

#[component]
pub(crate) fn TasksRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { TasksCard { app_state } }
}

#[component]
pub(crate) fn SystemLogsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { LogsPage { app_state } }
}

#[component]
pub(crate) fn SystemActivityRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ActivityLogPage { app_state } }
}

#[component]
pub(crate) fn SystemTelemetryRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { TelemetryPage { app_state } }
}

#[component]
pub(crate) fn SessionsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SessionsCard { app_state } }
}

#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
    navigator().replace(Route::DashboardRoute);
    rsx! {}
}
