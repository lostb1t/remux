use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use remux_sdks::remux::{
    AddTunerHost, AddonCatalogDto, AddonDto, AddonMetadata, AddonOption,
    AddonOptionType, AddonPresetRef, AdminSetPassword, AuthenticateUserByName,
    AuthenticationInfo, BaseItemDto, BrandingOptions, BulkChannelRequest, BulkChannels,
    ChannelEditorItem, CollectionFilter, CountryInfo, CreateAddon, CreateAddonRequest,
    CreateApiKey, CreateStreamGroup, CreateStreamGroupRequest, CreateUser,
    CreateVirtualFolder, CreateVirtualFolderPayload, DeleteAddon, DeleteApiKey,
    DeleteEpgSource, DeleteStreamGroup, DeleteTunerHost, DeleteUser,
    DeleteVirtualFolder, EncodingOptions, EpgSourceInfo, FilterMatchMode, FilterRule,
    GetAddonCatalogs, GetApiKeys, GetBrandingConfiguration,
    GetCertificationSuggestions, GetCountries, GetEncodingConfiguration, GetEpgSources,
    GetIptvChannelCountries, GetIptvChannels, GetItemCounts, GetItems,
    GetLocalSuggestions, GetParentalRatings, GetScheduledTasks, GetSessions,
    GetStartupConfiguration, GetStreamGroupPreview, GetSystemConfiguration,
    GetTagSuggestions, GetTunerHosts, GetUsers, HardwareAccelerationType, ItemCounts,
    JellyfinAuth, ListAddonKinds, ListAddons, ListStreamGroups, NumericOp,
    ParentalRating, PatchChannel, PatchChannelRequest, PatchItem, PatchItemPayload,
    PostStartupComplete, PostStartupConfiguration, PostStartupUser, PublicSystemInfo,
    SaveEpgSource, ServerConfiguration, SessionInfoDto, SetOp, SourceUrl, StartTask,
    StartupConfiguration, StartupUser, StopTask, StreamCodec, StreamFilter,
    StreamGroupDto, StreamGroupPreviewDto, StreamQuality, StreamResolution, StreamRule,
    TaskInfo, TaskTriggerInfo, TaskTriggerInfoType, TunerHostInfo, UpdateAddon,
    UpdateAddonCatalogRequest, UpdateAddonCatalogs, UpdateAddonRequest,
    UpdateBrandingConfiguration, UpdateEncodingConfiguration, UpdateStreamGroup,
    UpdateStreamGroupRequest, UpdateSystemConfiguration, UpdateTaskTriggers,
    UpdateUser, UpdateUserPolicy, UserDto, Username,
};
use remux_sdks::stremio::ResourceType;
use remux_sdks::{ClientError, RestClient};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn detect_image_content_type(bytes: &[u8]) -> &'static str {
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

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const THEME_CSS: Asset = asset!("/assets/theme.css");

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

#[derive(Clone)]
struct AppState {
    server: StoredServer,
    client: RestClient<JellyfinAuth>,
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
        self.server.id == other.server.id
    }
}

impl AppState {
    fn new(server: StoredServer) -> Self {
        let device_id = get_or_create_device_id();
        let auth =
            JellyfinAuth::new(&device_id).with_token(server.access_token.clone());
        let client = remux_sdks::remux::client(&server.manual_address)
            .unwrap_or_else(|_| panic!("invalid server url: {}", server.manual_address))
            .with_auth(auth);
        Self { server, client }
    }
}

/// Extracts HH:MM from a DateTime Display string ("2026-02-26 18:30:38 UTC").
fn fmt_time(dt: impl std::fmt::Display) -> String {
    let s = dt.to_string();
    s.chars().skip(11).take(5).collect()
}

fn get_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

fn browser_metadata_country_code() -> String {
    web_sys::window()
        .and_then(|w| w.navigator().language())
        .and_then(|language| {
            language
                .split(['-', '_'])
                .skip(1)
                .filter(|part| {
                    part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic())
                })
                .last()
                .map(|part| part.to_ascii_uppercase())
        })
        .unwrap_or_else(|| "US".to_string())
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
    let _ = LocalStorage::set(
        CREDENTIALS_KEY,
        &StoredCredentials {
            servers: vec![server],
        },
    );
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // None = still checking, Some(true) = wizard needed, Some(false) = normal flow
    let mut wizard_needed: Signal<Option<bool>> = use_signal(|| None);
    let mut logged_in = use_signal(|| get_stored_server().is_some());
    // Provide as context so DashboardLayout can call logout without prop-drilling.
    use_context_provider(|| logged_in);

    use_effect(move || {
        spawn(async move {
            let origin = get_origin();
            let needed = match remux_sdks::remux::client(&origin) {
                Ok(c) => c
                    .execute(PublicSystemInfo::default())
                    .await
                    .ok()
                    .map(|info| !info.startup_wizard_completed)
                    .unwrap_or(false),
                Err(_) => false,
            };
            wizard_needed.set(Some(needed));
        });
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: THEME_CSS }
        {match *wizard_needed.read() {
            None => rsx! {
                div { class: "login-page",
                    div { class: "login-card",
                        div { class: "login-header",
                            a { href: "/", class: "login-brand-label", "Remux" }
                            p { class: "connecting", "Starting up…" }
                        }
                    }
                }
            },
            Some(true) => rsx! {
                Wizard {
                    on_complete: move |_| {
                        wizard_needed.set(Some(false));
                    }
                }
            },
            Some(false) => rsx! {
                if *logged_in.read() {
                    Router::<Route> {}
                } else {
                    Login { on_login: move |_| logged_in.set(true) }
                }
            },
        }}
    }
}

#[component]
fn Login(on_login: EventHandler) -> Element {
    // None = probing, Some(url) = found, Some("") = not found / show field
    let mut server_url: Signal<Option<String>> = use_signal(|| None);
    let mut host_input = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            let origin = get_origin();
            let reachable = match remux_sdks::remux::client(&origin) {
                Ok(c) => c.execute(PublicSystemInfo::default()).await.is_ok(),
                Err(_) => false,
            };
            server_url.set(Some(if reachable { origin } else { String::new() }));
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();

        let url = match server_url.peek().clone() {
            Some(u) if !u.is_empty() => u,
            _ => {
                let h = host_input.peek().trim().to_string();
                if h.is_empty() {
                    error.set(Some("Please enter the server URL".into()));
                    return;
                }
                h
            }
        };

        let u = username.peek().clone();
        let p = password.peek().clone();
        let device_id = get_or_create_device_id();

        loading.set(true);
        error.set(None);

        spawn(async move {
            let client = match remux_sdks::remux::client(&url) {
                Ok(c) => c.with_auth(JellyfinAuth::new(&device_id)),
                Err(e) => {
                    error.set(Some(format!("Bad server URL: {e}")));
                    loading.set(false);
                    return;
                }
            };

            match client
                .execute(AuthenticateUserByName {
                    username: Some(u),
                    pw: Some(p),
                })
                .await
            {
                Ok(result) => {
                    if let (Some(token), Some(user)) =
                        (result.access_token, result.user)
                    {
                        store_credentials(StoredServer {
                            id: result.server_id,
                            name: "Remux".to_string(),
                            manual_address: url,
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
        div { class: "login-page",
            div { class: "login-card",
                div { class: "login-header",
                    span { class: "login-brand-label", "Remux" }
                    h1 { class: "login-title", "Admin Dashboard" }
                    p { class: "login-subtitle", "Sign in to continue" }
                }
                div { class: "login-body",
                    if server_url.read().is_none() {
                        p { class: "connecting", "Connecting…" }
                    } else {
                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }

                        form {
                            onsubmit: on_submit,
                            style: "display:flex;flex-direction:column;gap:14px;",

                            if server_url.read().as_deref() == Some("") {
                                div { class: "field",
                                    label { class: "field-label", r#for: "host", "Server URL" }
                                    input {
                                        id: "host",
                                        r#type: "url",
                                        class: "field-input",
                                        placeholder: "http://192.168.1.x:8096",
                                        value: "{host_input}",
                                        oninput: move |e| host_input.set(e.value()),
                                        required: true,
                                    }
                                }
                            }

                            div { class: "field",
                                label { class: "field-label", r#for: "username", "Username" }
                                input {
                                    id: "username",
                                    r#type: "text",
                                    class: "field-input",
                                    value: "{username}",
                                    oninput: move |e| username.set(e.value()),
                                    required: true,
                                    autocomplete: "username",
                                }
                            }
                            div { class: "field",
                                label { class: "field-label", r#for: "password", "Password" }
                                input {
                                    id: "password",
                                    r#type: "password",
                                    class: "field-input",
                                    value: "{password}",
                                    oninput: move |e| password.set(e.value()),
                                    autocomplete: "current-password",
                                }
                            }
                            button {
                                r#type: "submit",
                                class: "btn btn-primary login-btn",
                                disabled: *loading.read(),
                                if *loading.read() { "Signing in…" } else { "Sign In" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ServerInfoCard(app_state: AppState) -> Element {
    let mut server_info: Signal<Option<PublicSystemInfo>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state.client.clone();
        spawn(async move {
            match client.execute(PublicSystemInfo::default()).await {
                Ok(info) => {
                    server_info.set(Some(info));
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch server info: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Server" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if let Some(info) = server_info.read().as_ref() {
                    KvRow { label: "Name", value: info.server_name.clone() }
                    KvRow { label: "Version", value: info.remux_version.clone() }
                }
            }
        }
    }
}

#[component]
fn KvRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "kv-row",
            span { class: "kv-label", "{label}" }
            span { class: "kv-value", "{value}" }
        }
    }
}

#[component]
fn MediaStatsCard(app_state: AppState) -> Element {
    let mut counts: Signal<Option<ItemCounts>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state.client.clone();
        spawn(async move {
            match client.execute(GetItemCounts).await {
                Ok(c) => {
                    counts.set(Some(c));
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch media counts: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Library" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if let Some(c) = counts.read().as_ref() {
                    KvRow { label: "Movies", value: c.movie_count.to_string() }
                    KvRow { label: "Series", value: c.series_count.to_string() }
                    KvRow { label: "Episodes", value: c.episode_count.to_string() }
                    KvRow { label: "Albums", value: c.album_count.to_string() }
                    KvRow { label: "Tracks", value: c.song_count.to_string() }
                }
            }
        }
    }
}

#[component]
fn SessionsCard(app_state: AppState) -> Element {
    let mut sessions: Signal<Vec<SessionInfoDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state.client.clone();
        spawn(async move {
            match client
                .execute(GetSessions {
                    active_within_seconds: Some(960),
                })
                .await
            {
                Ok(s) => {
                    sessions.set(s);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch sessions: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Active Devices" }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if sessions.read().is_empty() {
                    div { class: "empty-state", "No active devices in the last 16 minutes" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for session in sessions.read().iter() {
                                div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                    div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                        div { class: "session-name",
                                            "{session.device_name.as_deref().unwrap_or(\"Unknown device\")}"
                                        }
                                        if let Some(item) = &session.now_playing_item {
                                            div { class: "session-playing",
                                                "▶ {item.name.as_deref().unwrap_or(\"Unknown\")}"
                                            }
                                        }
                                    }
                                    div { class: "shrink-0 px-3 py-[10px]",
                                        div { class: "session-user",
                                            "{session.user_name.as_deref().unwrap_or(\"Unknown\")}"
                                        }
                                    }
                                    div { class: "shrink-0 px-3 py-[10px]",
                                        if let Some(client_name) = &session.client {
                                            div { class: "session-client-badge",
                                                "{client_name}"
                                                if let Some(v) = &session.application_version {
                                                    " {v}"
                                                }
                                            }
                                        }
                                    }
                                    div { class: "shrink-0 px-3 py-[10px] text-right font-mono text-[var(--text-dim)] text-xs",
                                        "{fmt_time(session.last_activity_date)}"
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

fn trigger_label(t: &TaskTriggerInfo) -> String {
    let kind = t
        .r#type
        .as_deref()
        .and_then(|s| s.parse::<TaskTriggerInfoType>().ok());
    match kind {
        Some(TaskTriggerInfoType::StartupTrigger) => "On server startup".into(),
        Some(TaskTriggerInfoType::DailyTrigger) => {
            let ticks = t.time_of_day_ticks.unwrap_or(0);
            let total_secs = ticks / 10_000_000;
            let hour = total_secs / 3600;
            let min = (total_secs % 3600) / 60;
            format!("Daily at {:02}:{:02}", hour, min)
        }
        Some(TaskTriggerInfoType::WeeklyTrigger) => {
            let ticks = t.time_of_day_ticks.unwrap_or(0);
            let total_secs = ticks / 10_000_000;
            let hour = total_secs / 3600;
            let min = (total_secs % 3600) / 60;
            let day = t.day_of_week.as_deref().unwrap_or("Sunday");
            format!("Weekly on {} at {:02}:{:02}", day, hour, min)
        }
        Some(TaskTriggerInfoType::IntervalTrigger) => {
            let ticks = t.interval_ticks.unwrap_or(0);
            if ticks % 36_000_000_000 == 0 {
                format!("Every {} hour(s)", ticks / 36_000_000_000)
            } else {
                format!("Every {} minute(s)", ticks / 600_000_000)
            }
        }
        None => "Unknown".into(),
    }
}

#[component]
fn TaskTriggersModal(
    task: TaskInfo,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let mut triggers = use_signal(|| task.triggers.clone().unwrap_or_default());
    let mut new_type = use_signal(|| TaskTriggerInfoType::DailyTrigger);
    let mut new_hour = use_signal(|| "0".to_string());
    let mut new_min = use_signal(|| "0".to_string());
    let mut new_day = use_signal(|| "Sunday".to_string());
    let mut new_interval_value = use_signal(|| "24".to_string());
    let mut new_interval_unit = use_signal(|| "hours".to_string());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let task_id = task.id.clone();
    let task_name = task.name.clone();

    rsx! {
        h2 { class: "modal-title", "Triggers — {task_name}" }
        if let Some(desc) = task.description.as_deref().filter(|d| !d.is_empty()) {
            p { class: "text-muted", style: "margin-top: 0.25rem; margin-bottom: 1rem;", "{desc}" }
        }
        // Current triggers list
        for (i, trigger) in triggers.read().clone().into_iter().enumerate() {
            div {
                class: "field",
                style: "display: flex; align-items: center; justify-content: space-between;",
                span { "{trigger_label(&trigger)}" }
                button {
                    class: "btn btn-danger",
                    style: "padding: 2px 8px; font-size: 0.8rem;",
                    onclick: move |_| { triggers.write().remove(i); },
                    "×"
                }
            }
        }
        if triggers.read().is_empty() {
            p { class: "text-muted", "No triggers" }
        }

        hr {}

        h3 { "Add trigger" }
        div { class: "field",
            label { class: "field-label", "Type" }
            select {
                class: "select-input",
                value: "{new_type.read()}",
                onchange: move |evt| new_type.set(evt.value().parse().unwrap_or(TaskTriggerInfoType::DailyTrigger)),
                option { value: "{TaskTriggerInfoType::DailyTrigger}", "Daily" }
                option { value: "{TaskTriggerInfoType::WeeklyTrigger}", "Weekly" }
                option { value: "{TaskTriggerInfoType::IntervalTrigger}", "Interval" }
                option { value: "{TaskTriggerInfoType::StartupTrigger}", "On server startup" }
            }
        }
        if *new_type.read() == TaskTriggerInfoType::WeeklyTrigger {
            div { style: "display: flex; gap: 1rem;",
                div { class: "field",
                    label { class: "field-label", "Day" }
                    select {
                        class: "select-input",
                        value: "{new_day.read()}",
                        onchange: move |evt| new_day.set(evt.value()),
                        option { value: "Sunday", "Sunday" }
                        option { value: "Monday", "Monday" }
                        option { value: "Tuesday", "Tuesday" }
                        option { value: "Wednesday", "Wednesday" }
                        option { value: "Thursday", "Thursday" }
                        option { value: "Friday", "Friday" }
                        option { value: "Saturday", "Saturday" }
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Hour (0–23)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "23",
                        value: "{new_hour.read()}",
                        oninput: move |evt| new_hour.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Minute (0–59)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "59",
                        value: "{new_min.read()}",
                        oninput: move |evt| new_min.set(evt.value()),
                    }
                }
            }
        } else if *new_type.read() == TaskTriggerInfoType::DailyTrigger {
            div { style: "display: flex; gap: 1rem;",
                div { class: "field",
                    label { class: "field-label", "Hour (0–23)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "23",
                        value: "{new_hour.read()}",
                        oninput: move |evt| new_hour.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Minute (0–59)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "59",
                        value: "{new_min.read()}",
                        oninput: move |evt| new_min.set(evt.value()),
                    }
                }
            }
        } else if *new_type.read() == TaskTriggerInfoType::IntervalTrigger {
            div { style: "display: flex; gap: 1rem; align-items: flex-end;",
                div { class: "field",
                    label { class: "field-label", "Every" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        value: "{new_interval_value.read()}",
                        oninput: move |evt| new_interval_value.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Unit" }
                    select {
                        class: "select-input",
                        onchange: move |evt| new_interval_unit.set(evt.value()),
                        option { value: "minutes", selected: *new_interval_unit.read() == "minutes", "Minutes" }
                        option { value: "hours", selected: *new_interval_unit.read() == "hours", "Hours" }
                    }
                }
            }
        }
        button {
            class: "btn btn-secondary",
            onclick: move |_| {
                let kind = *new_type.read();
                let trigger = match kind {
                    TaskTriggerInfoType::StartupTrigger => TaskTriggerInfo {
                        r#type: Some(kind.to_string()),
                        ..Default::default()
                    },
                    TaskTriggerInfoType::IntervalTrigger => {
                        let val: i64 = new_interval_value.read().parse().unwrap_or(24).max(1);
                        let ticks_per_unit: i64 = if *new_interval_unit.read() == "minutes" {
                            600_000_000
                        } else {
                            36_000_000_000
                        };
                        TaskTriggerInfo {
                            r#type: Some(kind.to_string()),
                            interval_ticks: Some(val * ticks_per_unit),
                            ..Default::default()
                        }
                    }
                    TaskTriggerInfoType::DailyTrigger | TaskTriggerInfoType::WeeklyTrigger => {
                        let h: i64 = new_hour.read().parse().unwrap_or(0).clamp(0, 23);
                        let m: i64 = new_min.read().parse().unwrap_or(0).clamp(0, 59);
                        TaskTriggerInfo {
                            r#type: Some(kind.to_string()),
                            time_of_day_ticks: Some(h * 36_000_000_000 + m * 600_000_000),
                            day_of_week: (kind == TaskTriggerInfoType::WeeklyTrigger)
                                .then(|| new_day.read().clone()),
                            ..Default::default()
                        }
                    }
                };
                triggers.write().push(trigger);
            },
            "Add"
        }

        if let Some(e) = error.read().as_ref() {
            div { class: "alert-error", "{e}" }
        }

        div { class: "form-actions",
            button {
                class: "btn btn-ghost",
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
            button {
                class: "btn btn-primary",
                disabled: *saving.read(),
                onclick: move |_| {
                    let client = app_state.client.clone();
                    let tid = task_id.clone();
                    let t = triggers.read().clone();
                    saving.set(true);
                    error.set(None);
                    spawn(async move {
                        match client.execute(UpdateTaskTriggers { task_id: tid, triggers: t }).await {
                            Ok(_) => on_done.call(()),
                            Err(e) => {
                                saving.set(false);
                                error.set(Some(e.user_message()));
                            }
                        }
                    });
                },
                if *saving.read() { "Saving…" } else { "Save" }
            }
        }
    }
}

#[component]
fn TasksCard(
    app_state: AppState,
    #[props(default = false)] running_only: bool,
) -> Element {
    let mut tasks: Signal<Vec<TaskInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let mut selected_task: Signal<Option<TaskInfo>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read(); // only signal read — re-runs on start/stop, not on poll updates
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client
                .execute(GetScheduledTasks {
                    is_hidden: Some(false),
                })
                .await
            {
                Ok(t) => {
                    tasks.set(t);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch tasks: {e}"))),
            }
            loading.set(false);
        });
    });

    // Background polling — silently refreshes task list every 3 s
    let app_state_poll = app_state.clone();
    use_effect(move || {
        let client = app_state_poll.client.clone();
        spawn(async move {
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(t) = client
                    .execute(GetScheduledTasks {
                        is_hidden: Some(false),
                    })
                    .await
                {
                    tasks.set(t);
                }
            }
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", if running_only { "Running Tasks" } else { "Scheduled Tasks" } }
            }
            div { class: "card-body tight",
                if *loading.read() && tasks.read().is_empty() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else {
                    {
                        let visible: Vec<_> = tasks.read().iter()
                            .filter(|t| !running_only || t.state.as_deref() == Some("Running"))
                            .cloned()
                            .collect();
                        if visible.is_empty() {
                            rsx! {
                                div { class: "empty-state",
                                    if running_only { "No tasks currently running" } else { "No tasks found" }
                                }
                            }
                        } else if running_only {
                            rsx! {
                                div { class: "data-table-container",
                                    div { class: "row-list",
                                        for task in visible {
                                            TaskRow { key: "{task.id}", task }
                                        }
                                    }
                                }
                            }
                        } else {
                            // Group by category (BTreeMap → alphabetical order)
                            let mut groups: std::collections::BTreeMap<String, Vec<TaskInfo>> =
                                std::collections::BTreeMap::new();
                            for task in visible {
                                let cat = task.category.clone().unwrap_or_else(|| "Other".to_string());
                                groups.entry(cat).or_default().push(task);
                            }
                            let groups: Vec<(String, Vec<TaskInfo>)> = groups
                                .into_iter()
                                .map(|(cat, mut tasks)| {
                                    tasks.sort_by(|a, b| a.name.cmp(&b.name));
                                    (cat, tasks)
                                })
                                .collect();
                            rsx! {
                                for (cat, group_tasks) in groups {
                                    div { key: "{cat}", class: "task-group",
                                        div { class: "task-group-header", "{cat}" }
                                        div { class: "row-list",
                                            for task in group_tasks {
                                                TaskPageRow {
                                                    key: "{task.id}",
                                                    task: task.clone(),
                                                    show_category: false,
                                                    app_state: app_state.clone(),
                                                    on_refresh: move |_| {
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    },
                                                    on_edit: move |t: TaskInfo| selected_task.set(Some(t)),
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
        if let Some(task) = selected_task.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    TaskTriggersModal {
                        task,
                        app_state: app_state.clone(),
                        on_done: move |_| {
                            selected_task.set(None);
                            let v = *refresh.peek() + 1;
                            refresh.set(v);
                        },
                        on_cancel: move |_| selected_task.set(None),
                    }
                }
            }
        }
    }
}

/// Wraps `TaskRow` with start/stop controls; used on the Tasks page.
#[component]
fn TaskPageRow(
    task: TaskInfo,
    app_state: AppState,
    on_refresh: EventHandler,
    on_edit: EventHandler<TaskInfo>,
    #[props(default = true)] show_category: bool,
) -> Element {
    let start_id = task.id.clone();
    let stop_id = task.id.clone();
    let c_start = app_state.client.clone();
    let c_stop = app_state.client.clone();
    let task_for_edit = task.clone();

    rsx! {
        TaskRow {
            task,
            show_category,
            on_click: move |_| on_edit.call(task_for_edit.clone()),
            on_start: move |_| {
                let id = start_id.clone();
                let c  = c_start.clone();
                spawn(async move {
                    let _ = c.execute(StartTask { task_id: id }).await;
                    on_refresh.call(());
                });
            },
            on_stop: move |_| {
                let id = stop_id.clone();
                let c  = c_stop.clone();
                spawn(async move {
                    let _ = c.execute(StopTask { task_id: id }).await;
                    on_refresh.call(());
                });
            },
        }
    }
}

#[component]
fn TaskRow(
    task: TaskInfo,
    #[props(default = true)] show_category: bool,
    #[props(optional)] on_click: Option<EventHandler>,
    #[props(optional)] on_start: Option<EventHandler>,
    #[props(optional)] on_stop: Option<EventHandler>,
) -> Element {
    let state = task.state.as_deref().unwrap_or("Idle");
    let is_running = state == "Running";

    // Last result status shown when idle
    let last_status = task
        .last_execution_result
        .as_ref()
        .and_then(|r| r.status.as_deref())
        .unwrap_or("");

    let display_state = if is_running { state } else { last_status };
    let display_badge = if is_running {
        "task-badge task-badge-running"
    } else {
        match last_status {
            "Completed" => "task-badge task-badge-completed",
            "Failed" => "task-badge task-badge-failed",
            _ => "task-badge task-badge-idle",
        }
    };

    let has_controls = on_start.is_some() || on_stop.is_some();
    let clickable = on_click.is_some();

    rsx! {
        div {
            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
            style: if clickable { "cursor: pointer;" } else { "" },
            onclick: move |_| { if let Some(ref h) = on_click { h.call(()); } },
            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                div { class: "task-name", "{task.name}" }
                if show_category {
                    if let Some(cat) = &task.category {
                        div { class: "task-category", "{cat}" }
                    }
                }
                if is_running {
                    if let Some(pct) = task.current_progress_percentage {
                        div { class: "task-progress-bar",
                            div {
                                class: "task-progress-fill",
                                style: "width:{pct:.0}%",
                            }
                        }
                    }
                }
            }
            div { class: "shrink-0 px-3 py-[10px]",
                if !display_state.is_empty() {
                    span { class: "{display_badge}", "{display_state}" }
                }
            }
            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                if has_controls {
                    div { class: "task-actions",
                        if !is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Run now",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    if let Some(ref h) = on_start { h.call(()); }
                                },
                                "▶"
                            }
                        }
                        if is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Stop",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    if let Some(ref h) = on_stop { h.call(()); }
                                },
                                "■"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Routable, PartialEq, Debug)]
enum Route {
    #[layout(DashboardLayout)]
    // Standalone top
    #[route("/")]
    DashboardRoute,
    #[route("/addons")]
    AddonsRoute,
    // Content
    #[route("/content/library")]
    LibraryRoute,
    #[route("/content/iptv")]
    IptvRoute,
    // Streaming
    #[route("/streaming/groups")]
    StreamingGroupsRoute,
    #[route("/streaming/probing")]
    StreamingProbingRoute,
    #[route("/streaming/p2p")]
    StreamingP2pRoute,
    // Settings
    #[route("/settings/general")]
    SettingsGeneralRoute,
    #[route("/settings/playback")]
    SettingsPlaybackRoute,
    #[route("/settings/search")]
    SettingsSearchRoute,
    #[route("/settings/jellyfin-sync")]
    SettingsJellyfinSyncRoute,
    #[route("/settings/branding")]
    SettingsBrandingRoute,
    // Access
    #[route("/access/users")]
    AccessUsersRoute,
    #[route("/access/apikeys")]
    AccessApiKeysRoute,
    // Standalone bottom
    #[route("/tasks")]
    TasksRoute,
    #[route("/activity")]
    ActivityRoute,
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
fn NavItem(label: &'static str, active: bool, on_click: EventHandler) -> Element {
    rsx! {
        button {
            class: if active { "nav-item nav-item-active" } else { "nav-item" },
            onclick: move |_| on_click.call(()),
            "{label}"
        }
    }
}

#[component]
fn NavSubItem(label: &'static str, active: bool, on_click: EventHandler) -> Element {
    rsx! {
        button {
            class: if active { "nav-sub-item nav-sub-item-active" } else { "nav-sub-item" },
            onclick: move |_| on_click.call(()),
            "{label}"
        }
    }
}

#[component]
fn SidebarGroup(label: &'static str, active: bool, children: Element) -> Element {
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
fn DashboardLayout() -> Element {
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
        Route::AccessUsersRoute => "Users",
        Route::AccessApiKeysRoute => "API Keys",
        Route::TasksRoute => "Tasks",
        Route::ActivityRoute => "Activity",
        Route::NotFound { .. } => "",
    };

    rsx! {
        div { class: "layout",
            // Mobile backdrop
            if *sidebar_open.read() {
                div {
                    class: "sidebar-overlay",
                    onclick: move |_| sidebar_open.set(false),
                }
            }

            // Sidebar
            nav {
                class: if *sidebar_open.read() { "sidebar sidebar-open" } else { "sidebar" },

                div { class: "sidebar-brand",
                    h1 { class: "brand-title", style: "margin:0", "Remux" }
                }

                div { class: "sidebar-nav",
                    // Top standalone items
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
                            label: "Branding",
                            active: route == Route::SettingsBrandingRoute,
                            on_click: move |_| { navigator().push(Route::SettingsBrandingRoute); sidebar_open.set(false); },
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

                    // Bottom standalone items
                    NavItem {
                        label: "Activity",
                        active: route == Route::ActivityRoute,
                        on_click: move |_| { navigator().push(Route::ActivityRoute); sidebar_open.set(false); },
                    }
                }

                div { class: "sidebar-footer",
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

            // Main content
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

// Thin wrappers: pull AppState from context (provided by DashboardLayout)
// then pass as props to the real page components.

#[component]
fn DashboardRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! {
        ServerInfoCard { app_state: app_state.clone() }
        MediaStatsCard { app_state: app_state.clone() }
        SessionsCard { app_state: app_state.clone() }
        TasksCard { app_state: app_state.clone(), running_only: true }
    }
}

#[component]
fn AddonsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { AddonsPage { app_state } }
}

#[component]
fn LibraryRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { CollectionsPage { app_state } }
}

#[component]
fn IptvRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { IptvPage { app_state } }
}

#[component]
fn StreamingGroupsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { StreamGroupsCard { app_state } }
}

#[component]
fn StreamingProbingRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ProbeSettingsCard { app_state } }
}

#[component]
fn StreamingP2pRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { P2pSettingsCard { app_state } }
}

#[component]
fn SettingsGeneralRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ServerSettingsCard { app_state } }
}

#[component]
fn SettingsPlaybackRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { PlaybackSettingsCard { app_state } }
}

#[component]
fn SettingsSearchRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SearchSettingsCard { app_state } }
}

#[component]
fn SettingsJellyfinSyncRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { JellyfinImportCard { app_state } }
}

#[component]
fn SettingsBrandingRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { BrandingPage { app_state } }
}

#[component]
fn AccessUsersRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { UsersPage { app_state } }
}

#[component]
fn AccessApiKeysRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ApiKeysPage { app_state } }
}

#[component]
fn TasksRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { TasksCard { app_state } }
}

#[component]
fn ActivityRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SessionsCard { app_state } }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    navigator().replace(Route::DashboardRoute);
    rsx! {}
}

/// Which collection is currently being edited (None = creating new).
#[derive(Clone, Debug)]
enum FormMode {
    Create,
    Edit(BaseItemDto),
}

impl PartialEq for FormMode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FormMode::Create, FormMode::Create) => true,
            (FormMode::Edit(a), FormMode::Edit(b)) => a.id == b.id,
            _ => false,
        }
    }
}

#[component]
fn CollectionsPage(app_state: AppState) -> Element {
    let mut collections: Signal<Vec<BaseItemDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);
    let mut form_mode: Signal<Option<FormMode>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client
                .execute(GetItems {
                    include_item_types: vec!["BoxSet".to_string()],
                    recursive: false,
                })
                .await
            {
                Ok(result) => {
                    collections.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load collections: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Collections" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| form_mode.set(Some(FormMode::Create)),
                    "+ New Collection"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if collections.read().is_empty() {
                    div { class: "empty-state", "No collections yet" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for col in collections.read().clone() {
                                {
                                    let col_edit = col.clone();
                                    let col_del  = col.clone();
                                    let client_del = app_state.client.clone();
                                    let col_id_str = col.id.to_string();
                                    let name = col.name.clone().unwrap_or_default();
                                    let col_type_label = match col.collection_type.as_ref() {
                                        Some(ct) => match ct {
                                            remux_sdks::remux::CollectionType::Movies  => "Movies",
                                            remux_sdks::remux::CollectionType::Tvshows => "Shows",
                                            remux_sdks::remux::CollectionType::Music   => "Music",
                                            _ => "Unknown",
                                        },
                                        None => "Unknown",
                                    };
                                    let col_kind_label = match col.remux.as_ref().and_then(|r| r.collection_kind.as_ref()) {
                                        Some(remux_sdks::remux::RemuxCollectionKind::Smart)  => "Smart",
                                        Some(remux_sdks::remux::RemuxCollectionKind::Manual) => "Manual",
                                        None => "",
                                    };
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]", key: "{col_id_str}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "catalog-name", "{name}" }
                                                div { class: "catalog-meta",
                                                    span { class: "session-client-badge", "{col_type_label}" }
                                                    if !col_kind_label.is_empty() {
                                                        span { class: "session-client-badge", "{col_kind_label}" }
                                                    }
                                                    if col.remux.as_ref().and_then(|r| r.promoted).unwrap_or(false) {
                                                        span { class: "task-badge task-badge-running", "Library" }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px",
                                                    onclick: move |_| form_mode.set(Some(FormMode::Edit(col_edit.clone()))),
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                    onclick: move |_| {
                                                        let name = col_del.name.clone().unwrap_or_default();
                                                        let c    = client_del.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(DeleteVirtualFolder { name }).await;
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                    "Delete"
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

        if let Some(mode) = form_mode.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    CollectionForm {
                        mode,
                        app_state: app_state.clone(),
                        on_done: move |_| {
                            form_mode.set(None);
                            let v = *refresh.peek() + 1;
                            refresh.set(v);
                        },
                        on_cancel: move |_| form_mode.set(None),
                    }
                }
            }
        }
    }
}

#[component]
fn CollectionForm(
    mode: FormMode,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let is_edit = matches!(mode, FormMode::Edit(_));
    let existing: Option<BaseItemDto> = match &mode {
        FormMode::Edit(f) => Some(f.clone()),
        FormMode::Create => None,
    };

    let mut title = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.name.clone())
            .unwrap_or_default()
    });
    let mut promoted = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.remux.as_ref())
            .and_then(|r| r.promoted)
            .unwrap_or(false)
    });
    let mut col_type = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.collection_type.as_ref())
            .map(|ct| match ct {
                remux_sdks::remux::CollectionType::Movies => "movies".to_string(),
                remux_sdks::remux::CollectionType::Tvshows => "tvshows".to_string(),
                remux_sdks::remux::CollectionType::Music => "music".to_string(),
                _ => "movies".to_string(),
            })
            .unwrap_or_else(|| "movies".to_string())
    });
    let mut col_kind = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.remux.as_ref())
            .and_then(|r| r.collection_kind.as_ref())
            .map(|k| k.to_string())
            .unwrap_or_else(|| "smart".to_string())
    });
    // Smart filter rules
    let sf_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.remux.as_ref())
            .and_then(|r| r.smart_filter.as_ref())
            .map(|sf| sf.match_mode.clone())
            .unwrap_or(FilterMatchMode::All)
    });
    let sf_rules: Signal<Vec<FilterRule>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.remux.as_ref())
            .and_then(|r| r.smart_filter.as_ref())
            .map(|sf| sf.rules.clone())
            .unwrap_or_default()
    });
    let tags: Signal<Vec<String>> = use_signal(|| {
        existing
            .as_ref()
            .map(|f| f.tags.clone())
            .unwrap_or_default()
    });
    let mut saving = use_signal(|| false);
    let mut err = use_signal(|| Option::<String>::None);

    // Image upload state (edit mode only)
    let existing_image_tag = existing
        .as_ref()
        .and_then(|f| f.image_tags.as_ref())
        .and_then(|t| t.primary.clone());
    let existing_item_id = existing.as_ref().map(|f| f.id.to_string());
    let server_base = app_state.server.manual_address.clone();
    let current_image_url = existing_item_id
        .as_ref()
        .zip(existing_image_tag.as_ref())
        .map(|(id, tag)| format!("{server_base}/Items/{id}/Images/Primary?tag={tag}"));
    let mut pending_image_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
    let mut pending_image_preview: Signal<Option<String>> = use_signal(|| None);
    let mut has_image = use_signal(|| existing_image_tag.is_some());
    let client_for_delete = app_state.client.clone();

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let item_id = existing.as_ref().map(|f| f.id.to_string());
        let name = title.peek().clone();
        let ct = col_type.peek().clone();
        let ck = col_kind.peek().clone();
        let prm = *promoted.peek();
        let current_tags = tags.peek().clone();
        let smart_filter_payload = if ck == "smart" {
            Some(CollectionFilter {
                match_mode: sf_match.peek().clone(),
                rules: sf_rules.peek().clone(),
            })
        } else {
            None
        };
        saving.set(true);
        err.set(None);
        let pending_bytes = pending_image_bytes.peek().clone();
        spawn(async move {
            let result = if let Some(id) = item_id {
                let patch = client
                    .execute(PatchItem {
                        item_id: id.clone(),
                        payload: PatchItemPayload {
                            name: Some(name),
                            collection_type: Some(ct),
                            collection_kind: Some(ck),
                            smart_filter: smart_filter_payload,
                            promoted: Some(prm),
                            tags: Some(current_tags),
                        },
                    })
                    .await;
                if patch.is_ok() {
                    if let Some(bytes) = pending_bytes {
                        let ct = crate::detect_image_content_type(&bytes);
                        let _ = client
                            .execute(remux_sdks::remux::UploadItemImage {
                                item_id: id,
                                image_type: "Primary".to_string(),
                                bytes,
                                content_type: ct,
                            })
                            .await;
                    }
                }
                patch
            } else {
                client
                    .execute(CreateVirtualFolder {
                        payload: CreateVirtualFolderPayload {
                            name,
                            collection_type: Some(ct),
                            collection_kind: Some(ck),
                            promoted: Some(prm),
                        },
                    })
                    .await
                    .map(|_| ())
            };
            match result {
                Ok(_) => on_done.call(()),
                Err(e) => {
                    err.set(Some(e.user_message()));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        p { class: "modal-title",
            if is_edit { "Edit Collection" } else { "New Collection" }
        }

        form {
            onsubmit: on_submit,
            style: "display:flex;flex-direction:column;gap:14px",

            div { class: "field",
                label { class: "field-label", r#for: "col-title", "Title" }
                input {
                    id: "col-title",
                    r#type: "text",
                    class: "field-input",
                    required: true,
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "col-type", "Content Type" }
                select {
                    id: "col-type",
                    class: "select-input",
                    value: "{col_type}",
                    onchange: move |e| col_type.set(e.value()),
                    option { value: "movies",  "Movies"   }
                    option { value: "tvshows", "TV Shows" }
                    option { value: "music",   "Music"    }
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "col-kind", "Collection Kind" }
                select {
                    id: "col-kind",
                    class: "select-input",
                    value: "{col_kind}",
                    disabled: is_edit,
                    onchange: move |e| col_kind.set(e.value()),
                    option { value: "smart",  "Smart"  }
                    option { value: "manual", "Manual" }
                }
            }

            div { class: "field",
                label { class: "field-label", "Tags" }
                TagChipInput { tags }
            }

            if is_edit {
                div { class: "field",
                    label { class: "field-label", "Image" }
                    div { style: "display:flex;flex-direction:column;gap:8px",
                        // Preview: local pick takes priority over server image
                        if let Some(preview) = pending_image_preview.read().as_ref() {
                            img {
                                src: "{preview}",
                                style: "width:100%;max-height:180px;object-fit:cover;border-radius:6px;border:1px solid var(--border)",
                            }
                        } else if let Some(url) = &current_image_url {
                            if *has_image.read() {
                                img {
                                    src: "{url}",
                                    style: "width:100%;max-height:180px;object-fit:cover;border-radius:6px;border:1px solid var(--border)",
                                }
                            }
                        }
                        div { style: "display:flex;gap:8px;align-items:center",
                            label {
                                class: "btn btn-ghost",
                                style: "height:30px;font-size:.68rem;padding:0 10px;cursor:pointer",
                                input {
                                    r#type: "file",
                                    accept: "image/*",
                                    style: "display:none",
                                    onchange: move |e| {
                                        spawn(async move {
                                            let files_data = e.files();
                                            if let Some(file_data) = files_data.first() {
                                                if let Ok(raw) = file_data.read_bytes().await {
                                                    let bytes: Vec<u8> = raw.to_vec();
                                                    let ct = crate::detect_image_content_type(&bytes);
                                                    let b64 = base64::Engine::encode(
                                                        &base64::engine::general_purpose::STANDARD,
                                                        &bytes
                                                    );
                                                    let data_url = format!("data:{ct};base64,{b64}");
                                                    pending_image_preview.set(Some(data_url));
                                                    pending_image_bytes.set(Some(bytes));
                                                    has_image.set(true);
                                                }
                                            }
                                        });
                                    },
                                }
                                "Choose image"
                            }
                            if *has_image.read() {
                                button {
                                    r#type: "button",
                                    class: "btn btn-ghost",
                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                    onclick: {
                                        let item_id = existing_item_id.clone();
                                        let client = client_for_delete.clone();
                                        move |_| {
                                            let item_id = item_id.clone();
                                            let client = client.clone();
                                            spawn(async move {
                                                if let Some(id) = item_id {
                                                    let _ = client.execute(remux_sdks::remux::DeleteItemImage {
                                                        item_id: id,
                                                        image_type: "Primary".to_string(),
                                                    }).await;
                                                }
                                                pending_image_bytes.set(None);
                                                pending_image_preview.set(None);
                                                has_image.set(false);
                                            });
                                        }
                                    },
                                    "Remove image"
                                }
                            }
                        }
                    }
                }
            }

            div { class: "toggle-row",
                span { class: "toggle-label", "Promoted to Library" }
                label { class: "toggle",
                    input {
                        r#type: "checkbox",
                        checked: *promoted.read(),
                        onchange: move |e| promoted.set(e.checked()),
                    }
                    span { class: "toggle-track" }
                }
            }

            if col_kind.read().as_str() == "smart" {
                FilterRuleEditor { match_mode: sf_match, rules: sf_rules }
            }

            if let Some(e) = err.read().as_ref() {
                div { class: "alert-error", "{e}" }
            }

            div { class: "form-actions",
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
                button {
                    r#type: "submit",
                    class: "btn btn-primary",
                    disabled: *saving.read(),
                    if *saving.read() { "Saving…" } else { "Save" }
                }
            }
        }
    }
}

/// Simple chip input for editing a list of tags with autocomplete.
#[component]
fn TagChipInput(tags: Signal<Vec<String>>) -> Element {
    let app_state = use_context::<AppState>();
    let mut input_text: Signal<String> = use_signal(String::new);
    let mut suggestions: Signal<Vec<String>> = use_signal(Vec::new);
    let mut show_dropdown = use_signal(|| false);

    let client_fetch = app_state.client.clone();
    use_effect(move || {
        let q = input_text.read().clone();
        let client = client_fetch.clone();
        spawn(async move {
            if q.is_empty() {
                suggestions.set(vec![]);
                show_dropdown.set(false);
                return;
            }
            match client.execute(GetTagSuggestions { search_term: q }).await {
                Ok(v) => {
                    show_dropdown.set(!v.is_empty());
                    suggestions.set(v);
                }
                Err(_) => {}
            }
        });
    });

    let mut add_tag = move |tag: String| {
        let tag = tag.trim().to_string();
        if !tag.is_empty() && !tags.read().contains(&tag) {
            tags.write().push(tag);
        }
        input_text.set(String::new());
        suggestions.set(vec![]);
        show_dropdown.set(false);
    };

    rsx! {
        div { style: "position:relative",
            div { class: "chip-input",
                for (ci, chip) in tags.read().clone().into_iter().enumerate() {
                    span { class: "chip", key: "{ci}",
                        "{chip}"
                        button {
                            r#type: "button",
                            class: "chip-remove",
                            onclick: move |_| { tags.write().remove(ci); },
                            "×"
                        }
                    }
                }
                input {
                    r#type: "text",
                    class: "chip-text-input",
                    placeholder: if tags.read().is_empty() { "add tag…" } else { "" },
                    value: "{input_text}",
                    oninput: move |e| input_text.set(e.value()),
                    onkeydown: move |e| {
                        let key = e.key().to_string();
                        let text = input_text.read().replace(',', "").trim().to_string();
                        if (key == "Enter" || key == ",") && !text.is_empty() {
                            e.prevent_default();
                            add_tag(text);
                        } else if key == "Backspace" && input_text.read().is_empty() {
                            tags.write().pop();
                        }
                    },
                }
            }
            if *show_dropdown.read() {
                div { class: "autocomplete-dropdown",
                    for (si, suggestion) in suggestions.read().clone().into_iter().enumerate() {
                        div {
                            class: "autocomplete-item",
                            key: "{si}",
                            onmousedown: move |e| {
                                e.prevent_default();
                                add_tag(suggestion.clone());
                            },
                            "{suggestion}"
                        }
                    }
                }
            }
        }
    }
}

/// Extract the values vec from a set-type FilterRule without going through the string repr.
fn rule_values(rule: &FilterRule) -> Vec<String> {
    match rule {
        FilterRule::Genre { values, .. }
        | FilterRule::Certification { values, .. }
        | FilterRule::Tag { values, .. }
        | FilterRule::Studio { values, .. }
        | FilterRule::Country { values, .. }
        | FilterRule::Person { values, .. }
        | FilterRule::Collection { values, .. } => values.clone(),
        _ => vec![],
    }
}

/// Returns true for fields whose values are a set of strings (use ChipInput).
fn is_set_field(key: &str) -> bool {
    matches!(
        key,
        "genre"
            | "certification"
            | "tag"
            | "studio"
            | "country"
            | "person"
            | "collection"
    )
}

/// Fetch autocomplete suggestions for a field. Returns `(label, value)` pairs —
/// label is shown in the dropdown, value is what gets stored in the filter rule.
/// For most fields label == value; country is the exception (label = "United States", value = "US").
async fn fetch_suggestions(
    client: &RestClient<JellyfinAuth>,
    field: &str,
    query: &str,
) -> Vec<(String, String)> {
    match field {
        "genre" | "studio" | "person" => {
            let kind = match field {
                "genre" => "Genre",
                "studio" => "Studio",
                _ => "Person",
            };
            match client
                .execute(GetLocalSuggestions {
                    kind: kind.into(),
                    search_term: query.into(),
                })
                .await
            {
                Ok(r) => r
                    .items
                    .into_iter()
                    .filter_map(|i| i.name)
                    .map(|n| (n.clone(), n))
                    .collect(),
                Err(_) => vec![],
            }
        }
        "tag" => {
            match client
                .execute(GetTagSuggestions {
                    search_term: query.into(),
                })
                .await
            {
                Ok(tags) => tags.into_iter().map(|t| (t.clone(), t)).collect(),
                Err(_) => vec![],
            }
        }
        "collection" => {
            let Ok(addons) = client.execute(ListAddons).await else {
                return vec![];
            };
            let q = query.to_lowercase();
            let mut results = vec![];
            for addon in addons {
                let Ok(cats) = client.execute(GetAddonCatalogs { id: addon.id }).await
                else {
                    continue;
                };
                for cat in cats {
                    let label = format!("{}: {}", addon.name, cat.name);
                    if !label.to_lowercase().contains(&q) {
                        continue;
                    }
                    // Strip the "addon:" prefix — the stored value is "{uuid}:{local_id}"
                    let value = cat
                        .catalog_id
                        .strip_prefix("addon:")
                        .unwrap_or(&cat.catalog_id)
                        .to_string();
                    results.push((label, value));
                }
            }
            results
        }
        "certification" => {
            match client
                .execute(GetCertificationSuggestions {
                    search_term: query.into(),
                })
                .await
            {
                Ok(v) => v.into_iter().map(|s| (s.clone(), s)).collect(),
                Err(_) => vec![],
            }
        }
        "country" => {
            match client.execute(GetCountries).await {
                Ok(countries) => {
                    let q = query.to_lowercase();
                    countries
                        .into_iter()
                        .filter(|c| {
                            c.name.to_lowercase().contains(&q)
                                || c.two_letter_iso_region_name
                                    .to_lowercase()
                                    .contains(&q)
                        })
                        // label shows full name, value is the alpha-2 code stored in the DB
                        .map(|c| {
                            (
                                format!(
                                    "{} ({})",
                                    c.name, c.two_letter_iso_region_name
                                ),
                                c.two_letter_iso_region_name,
                            )
                        })
                        .take(25)
                        .collect()
                }
                Err(_) => vec![],
            }
        }
        _ => vec![],
    }
}

fn field_label(key: &str) -> &'static str {
    match key {
        "genre" => "Genre",
        "year" => "Year",
        "rating_audience" => "Audience Rating",
        "rating_critic" => "Critic Rating",
        "parental_rating" => "Max Parental Rating",
        "certification" => "Certification",
        "tag" => "Tag",
        "studio" => "Studio",
        "has_trailer" => "Has Trailer",
        "country" => "Country",
        "person" => "Person",
        "collection" => "Catalog",
        _ => "",
    }
}

/// Returns valid operators for a field key as (value, label) pairs.
fn ops_for_field(field_key: &str) -> Vec<(&'static str, &'static str)> {
    match field_key {
        "year" | "rating_audience" | "rating_critic" => {
            vec![("eq", "is"), ("not_eq", "is not"), ("gt", ">"), ("lt", "<")]
        }
        "parental_rating" | "has_trailer" => vec![],
        _ => vec![("is", "is"), ("is_not", "is not")],
    }
}

fn value_placeholder(field_key: &str) -> &'static str {
    match field_key {
        "year" => "2020",
        "rating_audience" | "rating_critic" => "7.5",
        "parental_rating" => "13",
        "certification" => "PG-13",
        "country" => "US",
        _ => "Action, Horror",
    }
}

/// Decompose a `FilterRule` into `(field_key, op_key, value_str)` for rendering.
fn rule_to_raw(rule: &FilterRule) -> (String, String, String) {
    match rule {
        FilterRule::Year { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("year".into(), op_str.into(), value.to_string())
        }
        FilterRule::RatingAudience { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("rating_audience".into(), op_str.into(), value.to_string())
        }
        FilterRule::RatingCritic { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("rating_critic".into(), op_str.into(), value.to_string())
        }
        FilterRule::ParentalRating { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("parental_rating".into(), op_str.into(), value.to_string())
        }
        FilterRule::Genre { op, values } => {
            ("genre".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Certification { op, values } => {
            ("certification".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Tag { op, values } => {
            ("tag".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Studio { op, values } => {
            ("studio".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Country { op, values } => {
            ("country".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Person { op, values } => {
            ("person".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::HasTrailer { value } => {
            ("has_trailer".into(), String::new(), value.to_string())
        }
        FilterRule::Collection { op, values } => {
            ("collection".into(), set_op_str(op), values.join(", "))
        }
    }
}

fn set_op_str(op: &SetOp) -> String {
    match op {
        SetOp::Is | SetOp::In => "is",
        SetOp::IsNot | SetOp::NotIn => "is_not",
    }
    .into()
}

/// Build a typed `FilterRule` from raw UI strings.
fn raw_to_rule(field: &str, op: &str, value_str: &str) -> FilterRule {
    // Set fields always use In/NotIn — single values are just a one-element vec.
    let set_op = match op {
        "is_not" => SetOp::NotIn,
        _ => SetOp::In,
    };
    let num_op = match op {
        "not_eq" => NumericOp::NotEq,
        "gt" => NumericOp::Gt,
        "lt" => NumericOp::Lt,
        _ => NumericOp::Eq,
    };
    let set_values = || -> Vec<String> {
        value_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    match field {
        "year" => FilterRule::Year {
            op: num_op,
            value: value_str.parse().unwrap_or(0),
        },
        "rating_audience" => FilterRule::RatingAudience {
            op: num_op,
            value: value_str.parse().unwrap_or(0.0),
        },
        "rating_critic" => FilterRule::RatingCritic {
            op: num_op,
            value: value_str.parse().unwrap_or(0.0),
        },
        "parental_rating" => FilterRule::ParentalRating {
            op: NumericOp::Lt,
            value: value_str.parse().unwrap_or(0),
        },
        "certification" => FilterRule::Certification {
            op: set_op,
            values: set_values(),
        },
        "tag" => FilterRule::Tag {
            op: set_op,
            values: set_values(),
        },
        "studio" => FilterRule::Studio {
            op: set_op,
            values: set_values(),
        },
        "country" => FilterRule::Country {
            op: set_op,
            values: set_values(),
        },
        "person" => FilterRule::Person {
            op: set_op,
            values: set_values(),
        },
        "has_trailer" => FilterRule::HasTrailer {
            value: value_str == "true",
        },
        "collection" => FilterRule::Collection {
            op: set_op,
            values: set_values(),
        },
        _ => FilterRule::Genre {
            op: set_op,
            values: set_values(),
        },
    }
}

#[component]
fn FilterRuleEditor(
    match_mode: Signal<FilterMatchMode>,
    rules: Signal<Vec<FilterRule>>,
) -> Element {
    let default_new_rule = FilterRule::Genre {
        op: SetOp::In,
        values: vec![],
    };
    rsx! {
        div {
            style: "background:var(--bg);border:1px solid var(--border);border-left:3px solid var(--info);border-radius:8px;padding:12px 14px",
            div { style: "display:flex;align-items:center;justify-content:space-between;margin-bottom:8px",
                label { class: "field-label", style: "margin:0", "Media Filters" }
                div { style: "display:flex;align-items:center;gap:8px",
                    span { style: "font-size:0.8rem;color:var(--text-muted)", "Match" }
                    select {
                        class: "select-input",
                        style: "padding:2px 6px;font-size:0.8rem",
                        value: if *match_mode.read() == FilterMatchMode::All { "all" } else { "any" },
                        onchange: move |e| {
                            match_mode.set(if e.value() == "any" {
                                FilterMatchMode::Any
                            } else {
                                FilterMatchMode::All
                            });
                        },
                        option { value: "all", "All (AND)" }
                        option { value: "any", "Any (OR)" }
                    }
                }
            }

            div { style: "display:flex;flex-direction:column;gap:6px",
                for (idx, rule) in rules.read().iter().enumerate() {
                    FilterRuleRow {
                        key: "{idx}",
                        idx,
                        rule: rule.clone(),
                        rules,
                    }
                }
            }

            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "margin-top:8px;font-size:0.85rem",
                onclick: move |_| {
                    rules.write().push(default_new_rule.clone());
                },
                "+ Add Filter"
            }
        }
    }
}

/// Multi-value chip input with server-side autocomplete dropdown.
#[component]
fn ChipInput(
    field_key: String,
    op_val: String,
    values: Vec<String>,
    idx: usize,
    rules: Signal<Vec<FilterRule>>,
) -> Element {
    let app_state = use_context::<AppState>();
    let mut input_text: Signal<String> = use_signal(String::new);
    // (label, value) — label shown in dropdown, value stored in rule
    let mut suggestions: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut show_dropdown = use_signal(|| false);
    let mut label_cache: Signal<std::collections::HashMap<String, String>> =
        use_signal(std::collections::HashMap::new);

    // On mount: pre-populate label_cache for collection values loaded from saved state.
    if field_key == "collection" {
        let client_init = app_state.client.clone();
        let values_init = values.clone();
        use_effect(move || {
            let uncached: Vec<String> = values_init
                .iter()
                .filter(|v| !label_cache.read().contains_key(*v))
                .cloned()
                .collect();
            if uncached.is_empty() {
                return;
            }
            let client = client_init.clone();
            spawn(async move {
                let Ok(addons) = client.execute(ListAddons).await else {
                    return;
                };
                for addon in addons {
                    let Ok(cats) =
                        client.execute(GetAddonCatalogs { id: addon.id }).await
                    else {
                        continue;
                    };
                    for cat in cats {
                        let value = cat
                            .catalog_id
                            .strip_prefix("addon:")
                            .unwrap_or(&cat.catalog_id)
                            .to_string();
                        if uncached.contains(&value) {
                            label_cache
                                .write()
                                .insert(value, format!("{}: {}", addon.name, cat.name));
                        }
                    }
                }
            });
        });
    }

    // Re-fetch suggestions whenever the typed text changes.
    let fk_fetch = field_key.clone();
    let client_fetch = app_state.client.clone();
    use_effect(move || {
        let q = input_text.read().clone();
        let fk = fk_fetch.clone();
        let client = client_fetch.clone();
        spawn(async move {
            if q.is_empty() {
                suggestions.set(vec![]);
                show_dropdown.set(false);
                return;
            }
            let result = fetch_suggestions(&client, &fk, &q).await;
            show_dropdown.set(!result.is_empty());
            suggestions.set(result);
        });
    });

    rsx! {
        div { style: "position:relative;flex:1.5",
            div { class: "chip-input",
                for (ci, chip) in values.iter().enumerate() {
                    {
                        let chip_display = label_cache.read().get(chip).cloned().unwrap_or(chip.clone());
                        let mut v = values.clone();
                        let fk = field_key.clone();
                        let op = op_val.clone();
                        rsx! {
                            span { class: "chip", key: "{ci}",
                                "{chip_display}"
                                button {
                                    r#type: "button",
                                    class: "chip-remove",
                                    onclick: move |_| {
                                        v.remove(ci);
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk, &op, &v.join(", "));
                                        }
                                    },
                                    "×"
                                }
                            }
                        }
                    }
                }
                {
                    let fk_kd = field_key.clone();
                    let op_kd = op_val.clone();
                    let vals_kd = values.clone();
                    rsx! {
                        input {
                            r#type: "text",
                            class: "chip-text-input",
                            placeholder: if values.is_empty() { "type to search…" } else { "" },
                            value: "{input_text}",
                            oninput: move |e| input_text.set(e.value()),
                            onkeydown: move |e| {
                                let key = e.key().to_string();
                                let text = input_text.read().replace(',', "");
                                let text = text.trim().to_string();
                                if (key == "Enter" || key == ",") && !text.is_empty() {
                                    e.prevent_default();
                                    let mut v = vals_kd.clone();
                                    if !v.contains(&text) { v.push(text); }
                                    if let Some(row) = rules.write().get_mut(idx) {
                                        *row = raw_to_rule(&fk_kd, &op_kd, &v.join(", "));
                                    }
                                    input_text.set(String::new());
                                    suggestions.set(vec![]);
                                    show_dropdown.set(false);
                                } else if key == "Backspace" && input_text.read().is_empty() {
                                    let mut v = vals_kd.clone();
                                    if !v.is_empty() {
                                        v.pop();
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk_kd, &op_kd, &v.join(", "));
                                        }
                                    }
                                }
                            },
                            onblur: move |_| show_dropdown.set(false),
                            onfocus: move |_| {
                                if !suggestions.read().is_empty() { show_dropdown.set(true); }
                            },
                        }
                    }
                }
            }
            if *show_dropdown.read() {
                div {
                    class: "chip-dropdown",
                    onmousedown: |e| e.prevent_default(),
                    for (label, value) in suggestions.read().clone() {
                        {
                            let mut v = values.clone();
                            let fk = field_key.clone();
                            let op = op_val.clone();
                            rsx! {
                                div {
                                    class: "chip-dropdown-item",
                                    key: "{value}",
                                    onmousedown: move |_| {
                                        label_cache.write().insert(value.clone(), label.clone());
                                        if !v.contains(&value) { v.push(value.clone()); }
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk, &op, &v.join(", "));
                                        }
                                        input_text.set(String::new());
                                        suggestions.set(vec![]);
                                        show_dropdown.set(false);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FilterRuleRow(
    idx: usize,
    rule: FilterRule,
    rules: Signal<Vec<FilterRule>>,
) -> Element {
    let app_state = use_context::<AppState>();
    let mut parental_ratings: Signal<Vec<ParentalRating>> = use_signal(Vec::new);
    use_effect(move || {
        let client = app_state.client.clone();
        spawn(async move {
            if let Ok(ratings) = client.execute(GetParentalRatings).await {
                parental_ratings.set(ratings);
            }
        });
    });

    let (field_val, op_val, value_val) = rule_to_raw(&rule);
    let ops = ops_for_field(&field_val);
    let is_trailer = field_val == "has_trailer";
    let is_parental_rating = field_val == "parental_rating";
    let hide_operator = is_trailer || is_parental_rating;

    // Clones for closures that capture by move
    let fv1 = field_val.clone();
    let fv2 = field_val.clone();
    let ov1 = op_val.clone();
    let vv1 = value_val.clone();
    let vv2 = value_val.clone();

    rsx! {
        div { style: "display:flex;align-items:center;gap:6px",
            // Field selector
            select {
                class: "select-input",
                style: "flex:1.2",
                value: "{field_val}",
                onchange: move |e| {
                    let new_field = e.value();
                    let default_op = ops_for_field(&new_field).first().map(|(v, _)| *v).unwrap_or("");
                    if let Some(row) = rules.write().get_mut(idx) {
                        *row = raw_to_rule(&new_field, default_op, "");
                    }
                },
                option { value: "genre",           selected: field_val == "genre",           { field_label("genre") } }
                option { value: "year",            selected: field_val == "year",            { field_label("year") } }
                option { value: "rating_audience", selected: field_val == "rating_audience", { field_label("rating_audience") } }
                option { value: "rating_critic",   selected: field_val == "rating_critic",   { field_label("rating_critic") } }
                option { value: "parental_rating", selected: field_val == "parental_rating", { field_label("parental_rating") } }
                option { value: "tag",             selected: field_val == "tag",             { field_label("tag") } }
                option { value: "studio",          selected: field_val == "studio",          { field_label("studio") } }
                option { value: "has_trailer",     selected: field_val == "has_trailer",     { field_label("has_trailer") } }
                option { value: "country",         selected: field_val == "country",         { field_label("country") } }
                option { value: "person",          selected: field_val == "person",          { field_label("person") } }
                option { value: "collection",      selected: field_val == "collection",      { field_label("collection") } }
            }
            // Operator selector (hidden for has_trailer which has no operator)
            if !hide_operator {
                select {
                    class: "select-input",
                    style: "flex:1",
                    value: "{op_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv1, &e.value(), &vv1);
                        }
                    },
                    for (op_v, op_l) in ops.iter() {
                        option { value: *op_v, selected: op_val == *op_v, "{op_l}" }
                    }
                }
            }
            // Value input — dropdown for has_trailer/parental_rating, chip input for set fields, text for numeric
            if is_trailer {
                select {
                    class: "select-input",
                    style: "flex:1",
                    value: "{value_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule("has_trailer", "", &e.value());
                        }
                    },
                    option { value: "true",  selected: value_val == "true",  "Yes" }
                    option { value: "false", selected: value_val == "false", "No" }
                }
            } else if is_parental_rating {
                select {
                    class: "select-input",
                    style: "flex:1.5",
                    value: "{value_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv2, "lt", &e.value());
                        }
                    },
                    option { value: "", selected: value_val.is_empty(), disabled: true, "Select rating" }
                    for rating in parental_ratings.read().iter().filter(|r| r.value.is_some()) {
                        option {
                            value: "{rating.value.unwrap_or_default()}",
                            selected: value_val == rating.value.unwrap_or_default().to_string(),
                            "{rating.name}"
                        }
                    }
                }
            } else if is_set_field(&field_val) {
                ChipInput {
                    field_key: field_val.clone(),
                    op_val: op_val.clone(),
                    values: rule_values(&rule),
                    idx,
                    rules,
                }
            } else {
                input {
                    class: "field-input",
                    style: "flex:1.5",
                    r#type: "text",
                    placeholder: value_placeholder(&fv2),
                    value: "{vv2}",
                    oninput: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv2, &ov1, &e.value());
                        }
                    },
                }
            }
            // Remove button
            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "padding:4px 8px;color:var(--text-muted)",
                onclick: move |_| {
                    let mut r = rules.write();
                    if idx < r.len() {
                        r.remove(idx);
                    }
                },
                "✕"
            }
        }
    }
}

#[derive(Clone)]
enum UserFormMode {
    Create,
    Edit(UserDto),
}

impl PartialEq for UserFormMode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Create, Self::Create) => true,
            (Self::Edit(a), Self::Edit(b)) => a.id == b.id,
            _ => false,
        }
    }
}

#[component]
fn IptvPage(app_state: AppState) -> Element {
    // "sources" or "channels"
    let mut active_tab = use_signal(|| "sources".to_string());

    rsx! {
        if active_tab.read().as_str() == "sources" {
            IptvSourcesTab { app_state: app_state.clone() }
        }
        if active_tab.read().as_str() == "channels" {
            IptvChannelsTab { app_state: app_state.clone() }
        }
        // Tab selector rendered above both panels
        div { class: "card", style: "order:-1",
            div { class: "card-header",
                span { class: "card-title", "IPTV" }
                div { class: "tab-group",
                    button {
                        class: if active_tab.read().as_str() == "sources" { "tab-btn active" } else { "tab-btn" },
                        onclick: move |_| active_tab.set("sources".to_string()),
                        "Sources"
                    }
                    button {
                        class: if active_tab.read().as_str() == "channels" { "tab-btn active" } else { "tab-btn" },
                        onclick: move |_| active_tab.set("channels".to_string()),
                        "Channels"
                    }
                }
            }
        }
    }
}

#[component]
fn IptvSourcesTab(app_state: AppState) -> Element {
    let mut ch_sources: Signal<Vec<TunerHostInfo>> = use_signal(Vec::new);
    let mut epg_sources: Signal<Vec<EpgSourceInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Channel source form
    let mut show_ch_form = use_signal(|| false);
    let mut ch_edit_id: Signal<Option<String>> = use_signal(|| None);
    let mut ch_form_type = use_signal(|| "m3u".to_string());
    let mut ch_form_name = use_signal(String::new);
    let mut ch_form_url = use_signal(String::new);
    let mut ch_form_username = use_signal(String::new);
    let mut ch_form_password = use_signal(String::new);
    let mut ch_saving = use_signal(|| false);
    let mut ch_save_error: Signal<Option<String>> = use_signal(|| None);

    // EPG source form
    let mut show_epg_form = use_signal(|| false);
    let mut epg_edit_id: Signal<Option<String>> = use_signal(|| None);
    let mut epg_form_name = use_signal(String::new);
    let mut epg_form_url = use_signal(String::new);
    let mut epg_saving = use_signal(|| false);
    let mut epg_save_error: Signal<Option<String>> = use_signal(|| None);

    let mut refreshing = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            let r1 = client.execute(GetTunerHosts).await;
            let r2 = client.execute(GetEpgSources).await;
            match (r1, r2) {
                (Ok(s), Ok(e)) => {
                    ch_sources.set(s);
                    epg_sources.set(e);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load: {e}")))
                }
            }
            loading.set(false);
        });
    });

    let mut reset_ch_form = move || {
        ch_edit_id.set(None);
        ch_form_type.set("m3u".to_string());
        ch_form_name.set(String::new());
        ch_form_url.set(String::new());
        ch_form_username.set(String::new());
        ch_form_password.set(String::new());
        ch_save_error.set(None);
    };

    let mut reset_epg_form = move || {
        epg_edit_id.set(None);
        epg_form_name.set(String::new());
        epg_form_url.set(String::new());
        epg_save_error.set(None);
    };

    let loading_v = *loading.read();
    let error_v = error.read().clone();

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Channel Sources" }
                div { style: "display:flex;gap:8px",
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        disabled: *refreshing.read(),
                        onclick: {
                            let client = app_state.client.clone();
                            move |_| {
                                refreshing.set(true);
                                let c = client.clone();
                                spawn(async move {
                                    let _ = c.execute(StartTask { task_id: "refreshiptv".to_string() }).await;
                                    refreshing.set(false);
                                });
                            }
                        },
                        if *refreshing.read() { "Refreshing…" } else { "Refresh Now" }
                    }
                    button {
                        class: "btn btn-primary",
                        style: "height:32px;font-size:.68rem",
                        onclick: move |_| { reset_ch_form(); show_ch_form.set(true); },
                        "+ Add"
                    }
                }
            }

            div { class: "card-body tight",
                if loading_v {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = &error_v {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if ch_sources.read().is_empty() {
                    div { class: "empty-state", "No channel sources. Add an M3U or Xtream Codes source." }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for source in ch_sources.read().clone() {
                                {
                                    let source_id = source.id.clone().unwrap_or_default();
                                    let name = source.friendly_name.clone().unwrap_or_else(|| "Unnamed".to_string());
                                    let url = source.url.clone().unwrap_or_default();
                                    let source_type = source.type_.clone().unwrap_or_else(|| "m3u".to_string());
                                    let type_label = if source_type == "xtream" { "Xtream" } else { "M3U" };
                                    let username = source.username.clone().unwrap_or_default();
                                    let client_del = app_state.client.clone();
                                    let sid = source_id.clone();
                                    let src_clone = source.clone();
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]", key: "{source_id}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "catalog-name", "{name}" }
                                                div { class: "catalog-meta",
                                                    span { class: "session-client-badge", "{type_label}" }
                                                    if source_type == "xtream" {
                                                        span { class: "session-client-badge", style: "background:var(--accent-muted)", "EPG" }
                                                    }
                                                    if source_type == "xtream" && !username.is_empty() {
                                                        span { style: "font-size:.72rem;opacity:.6", "user: {username}" }
                                                    } else {
                                                        span { style: "font-size:.72rem;opacity:.6", "{url}" }
                                                    }
                                                }
                                            }
                                            div { style: "display:flex;gap:4px;padding-right:8px",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:28px;font-size:.68rem",
                                                    onclick: move |_| {
                                                        ch_edit_id.set(src_clone.id.clone());
                                                        ch_form_type.set(src_clone.type_.clone().unwrap_or_else(|| "m3u".to_string()));
                                                        ch_form_name.set(src_clone.friendly_name.clone().unwrap_or_default());
                                                        ch_form_url.set(src_clone.url.clone().unwrap_or_default());
                                                        ch_form_username.set(src_clone.username.clone().unwrap_or_default());
                                                        ch_form_password.set(String::new());
                                                        ch_save_error.set(None);
                                                        show_ch_form.set(true);
                                                    },
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:28px;font-size:.68rem;color:var(--error)",
                                                    onclick: move |_| {
                                                        let c = client_del.clone();
                                                        let id = sid.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(DeleteTunerHost { id }).await;
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Channel source form
                if *show_ch_form.read() {
                    div { class: "form-section", style: "padding:16px;border-top:1px solid var(--border)",
                        div { class: "form-title", if ch_edit_id.read().is_some() { "Edit Channel Source" } else { "Add Channel Source" } }

                        // Type selector
                        div { class: "form-group",
                            label { class: "form-label", "Type" }
                            div { class: "tab-group",
                                button {
                                    class: if ch_form_type.read().as_str() == "m3u" { "tab-btn active" } else { "tab-btn" },
                                    onclick: move |_| ch_form_type.set("m3u".to_string()),
                                    "M3U"
                                }
                                button {
                                    class: if ch_form_type.read().as_str() == "xtream" { "tab-btn active" } else { "tab-btn" },
                                    onclick: move |_| ch_form_type.set("xtream".to_string()),
                                    "Xtream Codes"
                                }
                            }
                        }

                        div { class: "form-group",
                            label { class: "form-label", "Name" }
                            input {
                                class: "form-input",
                                value: "{ch_form_name.read()}",
                                placeholder: "IPTV",
                                oninput: move |e| ch_form_name.set(e.value()),
                            }
                        }

                        if ch_form_type.read().as_str() == "m3u" {
                            div { class: "form-group",
                                label { class: "form-label", "M3U URL" }
                                input {
                                    class: "form-input",
                                    value: "{ch_form_url.read()}",
                                    placeholder: "http://…/playlist.m3u",
                                    oninput: move |e| ch_form_url.set(e.value()),
                                }
                            }
                        } else {
                            div { class: "form-group",
                                label { class: "form-label", "Server URL" }
                                input {
                                    class: "form-input",
                                    value: "{ch_form_url.read()}",
                                    placeholder: "http://provider:8080",
                                    oninput: move |e| ch_form_url.set(e.value()),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Username" }
                                input {
                                    class: "form-input",
                                    value: "{ch_form_username.read()}",
                                    oninput: move |e| ch_form_username.set(e.value()),
                                }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Password" }
                                input {
                                    r#type: "password",
                                    class: "form-input",
                                    value: "{ch_form_password.read()}",
                                    placeholder: if ch_edit_id.read().is_some() { "leave blank to keep existing" } else { "" },
                                    oninput: move |e| ch_form_password.set(e.value()),
                                }
                            }
                        }

                        if let Some(err) = ch_save_error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }

                        div { class: "form-actions",
                            button {
                                class: "btn btn-ghost",
                                onclick: move |_| { show_ch_form.set(false); },
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: *ch_saving.read(),
                                onclick: {
                                    let client = app_state.client.clone();
                                    move |_| {
                                        let name = ch_form_name.peek().clone();
                                        let url = ch_form_url.peek().clone();
                                        let ty = ch_form_type.peek().clone();
                                        let username = ch_form_username.peek().clone();
                                        let password = ch_form_password.peek().clone();
                                        let edit_id = ch_edit_id.peek().clone();

                                        if url.is_empty() {
                                            ch_save_error.set(Some("URL is required".into()));
                                            return;
                                        }
                                        if ty == "xtream" && username.is_empty() {
                                            ch_save_error.set(Some("Username is required for Xtream".into()));
                                            return;
                                        }

                                        ch_saving.set(true);
                                        ch_save_error.set(None);
                                        let c = client.clone();
                                        spawn(async move {
                                            let info = TunerHostInfo {
                                                id: edit_id,
                                                friendly_name: Some(if name.is_empty() { "IPTV".to_string() } else { name }),
                                                url: Some(url),
                                                type_: Some(ty.clone()),
                                                username: if ty == "xtream" { Some(username) } else { None },
                                                password: if ty == "xtream" && !password.is_empty() { Some(password) } else { None },
                                                ..Default::default()
                                            };
                                            match c.execute(AddTunerHost { info }).await {
                                                Ok(_) => {
                                                    show_ch_form.set(false);
                                                    let v = *refresh.peek() + 1;
                                                    refresh.set(v);
                                                }
                                                Err(e) => ch_save_error.set(Some(e.user_message())),
                                            }
                                            ch_saving.set(false);
                                        });
                                    }
                                },
                                if *ch_saving.read() { "Saving…" } else { "Save" }
                            }
                        }
                    }
                }
            }
        }

        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "EPG Sources" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| { reset_epg_form(); show_epg_form.set(true); },
                    "+ Add"
                }
            }

            div { class: "card-body tight",
                if loading_v {
                    span { class: "loading-text", "Loading…" }
                } else if epg_sources.read().is_empty() {
                    div { class: "empty-state", "No EPG sources. Add an XMLTV URL to get program guide data." }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for epg in epg_sources.read().clone() {
                                {
                                    let eid = epg.id.clone().unwrap_or_default();
                                    let ename = epg.name.clone();
                                    let eurl = epg.url.clone();
                                    let client_del = app_state.client.clone();
                                    let epg_clone = epg.clone();
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]", key: "{eid}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "catalog-name", "{ename}" }
                                                div { class: "catalog-meta",
                                                    span { style: "font-size:.72rem;opacity:.6", "{eurl}" }
                                                }
                                            }
                                            div { style: "display:flex;gap:4px;padding-right:8px",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:28px;font-size:.68rem",
                                                    onclick: move |_| {
                                                        epg_edit_id.set(epg_clone.id.clone());
                                                        epg_form_name.set(epg_clone.name.clone());
                                                        epg_form_url.set(epg_clone.url.as_ref().to_string());
                                                        epg_save_error.set(None);
                                                        show_epg_form.set(true);
                                                    },
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:28px;font-size:.68rem;color:var(--error)",
                                                    onclick: move |_| {
                                                        let c = client_del.clone();
                                                        let id = eid.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(DeleteEpgSource { id }).await;
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // EPG source form
                if *show_epg_form.read() {
                    div { class: "form-section", style: "padding:16px;border-top:1px solid var(--border)",
                        div { class: "form-title", if epg_edit_id.read().is_some() { "Edit EPG Source" } else { "Add EPG Source" } }

                        div { class: "form-group",
                            label { class: "form-label", "Name" }
                            input {
                                class: "form-input",
                                value: "{epg_form_name.read()}",
                                oninput: move |e| epg_form_name.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "XMLTV URL" }
                            input {
                                class: "form-input",
                                value: "{epg_form_url.read()}",
                                placeholder: "http://…/xmltv.php",
                                oninput: move |e| epg_form_url.set(e.value()),
                            }
                        }

                        if let Some(err) = epg_save_error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }

                        div { class: "form-actions",
                            button {
                                class: "btn btn-ghost",
                                onclick: move |_| { show_epg_form.set(false); },
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: *epg_saving.read(),
                                onclick: {
                                    let client = app_state.client.clone();
                                    move |_| {
                                        let name = epg_form_name.peek().clone();
                                        let url = epg_form_url.peek().clone();
                                        let edit_id = epg_edit_id.peek().clone();

                                        let url = match SourceUrl::try_new(url) {
                                            Ok(u) => u,
                                            Err(_) => {
                                                epg_save_error.set(Some("URL is required".into()));
                                                return;
                                            }
                                        };

                                        epg_saving.set(true);
                                        epg_save_error.set(None);
                                        let c = client.clone();
                                        spawn(async move {
                                            let info = EpgSourceInfo { id: edit_id, name, url };
                                            match c.execute(SaveEpgSource { info }).await {
                                                Ok(_) => {
                                                    show_epg_form.set(false);
                                                    let v = *refresh.peek() + 1;
                                                    refresh.set(v);
                                                }
                                                Err(e) => epg_save_error.set(Some(e.user_message())),
                                            }
                                            epg_saving.set(false);
                                        });
                                    }
                                },
                                if *epg_saving.read() { "Saving…" } else { "Save" }
                            }
                        }
                    }
                }
            }
        }
    }
}

const PAGE_SIZE: u32 = 50;

#[component]
fn IptvChannelsTab(app_state: AppState) -> Element {
    let mut channels: Signal<Vec<ChannelEditorItem>> = use_signal(Vec::new);
    let mut total: Signal<usize> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut page = use_signal(|| 0_u32);
    // committed search (triggers fetch); typed search (live input)
    let mut search_committed = use_signal(String::new);
    let mut search_input = use_signal(String::new);
    let mut bulk_working = use_signal(|| false);
    // "all" | "true" | "false"
    let mut enabled_filter = use_signal(|| "all".to_string());
    let mut country_filter = use_signal(String::new);
    let mut countries: Signal<Vec<String>> = use_signal(Vec::new);
    // "order" | "name"
    let mut sort_mode = use_signal(|| "order".to_string());
    let mut show_filter_modal = use_signal(|| false);

    // Load distinct country codes once on mount
    let app_state_countries = app_state.clone();
    use_effect(move || {
        let client = app_state_countries.client.clone();
        spawn(async move {
            if let Ok(cs) = client.execute(GetIptvChannelCountries).await {
                countries.set(cs);
            }
        });
    });

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let p = *page.read();
        let s = search_committed.read().clone();
        let ef = enabled_filter.read().clone();
        let cf = country_filter.read().clone();
        let sm = sort_mode.read().clone();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            let enabled = match ef.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            match client
                .execute(GetIptvChannels {
                    limit: PAGE_SIZE,
                    offset: p * PAGE_SIZE,
                    search: s,
                    enabled,
                    country: cf,
                    sort: sm,
                })
                .await
            {
                Ok(r) => {
                    total.set(r.total_record_count);
                    channels.set(r.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load channels: {e}"))),
            }
            loading.set(false);
        });
    });

    let total_v = *total.read();
    let page_v = *page.read();
    let total_pages = total_v.div_ceil(PAGE_SIZE as usize) as u32;

    let mut do_search = move || {
        let s = search_input.peek().clone();
        search_committed.set(s);
        page.set(0);
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Channels" }
                if total_v > 0 {
                    span { style: "font-size:.75rem;opacity:.5;margin-left:8px", "{total_v} total" }
                }
                div { style: "display:flex;gap:8px;align-items:center;margin-left:auto",
                    // Filters / Sort modal trigger
                    {
                        let filters_active = enabled_filter.read().as_str() != "all"
                            || !country_filter.read().is_empty()
                            || sort_mode.read().as_str() != "order"
                            || !search_committed.read().is_empty();
                        rsx! {
                            button {
                                class: if filters_active { "btn btn-primary" } else { "btn btn-ghost" },
                                style: "height:32px;font-size:.68rem",
                                onclick: move |_| show_filter_modal.set(true),
                                if filters_active { "Filters ●" } else { "Filters" }
                            }
                        }
                    }
                    // Enable all / Disable all — server-side bulk op
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        disabled: *bulk_working.read() || total_v == 0,
                        onclick: {
                            let client = app_state.client.clone();
                            move |_| {
                                let search = search_committed.peek().clone();
                                bulk_working.set(true);
                                let c = client.clone();
                                spawn(async move {
                                    let _ = c.execute(BulkChannels {
                                        request: BulkChannelRequest { enabled: true, search: if search.is_empty() { None } else { Some(search) } },
                                    }).await;
                                    bulk_working.set(false);
                                    // re-fetch to reflect new state
                                    let s = search_committed.peek().clone();
                                    let p = *page.peek();
                                    let ef = enabled_filter.peek().clone();
                                    let cf = country_filter.peek().clone();
                                    let sm = sort_mode.peek().clone();
                                    let enabled = match ef.as_str() {
                                        "true" => Some(true),
                                        "false" => Some(false),
                                        _ => None,
                                    };
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s, enabled, country: cf, sort: sm }).await {
                                        total.set(r.total_record_count);
                                        channels.set(r.items);
                                    }
                                    loading.set(false);
                                });
                            }
                        },
                        if *bulk_working.read() { "Working…" } else { "Enable All" }
                    }
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        disabled: *bulk_working.read() || total_v == 0,
                        onclick: {
                            let client = app_state.client.clone();
                            move |_| {
                                let search = search_committed.peek().clone();
                                bulk_working.set(true);
                                let c = client.clone();
                                spawn(async move {
                                    let _ = c.execute(BulkChannels {
                                        request: BulkChannelRequest { enabled: false, search: if search.is_empty() { None } else { Some(search) } },
                                    }).await;
                                    bulk_working.set(false);
                                    let s = search_committed.peek().clone();
                                    let p = *page.peek();
                                    let ef = enabled_filter.peek().clone();
                                    let cf = country_filter.peek().clone();
                                    let sm = sort_mode.peek().clone();
                                    let enabled = match ef.as_str() {
                                        "true" => Some(true),
                                        "false" => Some(false),
                                        _ => None,
                                    };
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s, enabled, country: cf, sort: sm }).await {
                                        total.set(r.total_record_count);
                                        channels.set(r.items);
                                    }
                                    loading.set(false);
                                });
                            }
                        },
                        if *bulk_working.read() { "Working…" } else { "Disable All" }
                    }
                }
            }

            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if channels.read().is_empty() {
                    div { class: "empty-state",
                        if total_v == 0
                            && search_committed.read().is_empty()
                            && enabled_filter.read().as_str() == "all"
                            && country_filter.read().is_empty()
                        {
                            "No channels yet. Run a refresh after adding channel sources."
                        } else {
                            "No channels match your filters."
                        }
                    }
                } else {
                    div { class: "data-table-container",
                        // Column header
                        div { class: "flex items-center px-3 py-1 border-b border-[var(--border)]",
                            style: "font-size:.72rem;opacity:.5;font-weight:600;gap:8px",
                            div { style: "width:32px", "On" }
                            div { class: "flex-1", "Name / Display Name" }
                            div { style: "width:80px;text-align:right", "Ch#" }
                        }
                        div { class: "row-list",
                            for ch in channels.read().clone() {
                                {
                                    let id = ch.id.clone();
                                    let client1 = app_state.client.clone();
                                    let client2 = app_state.client.clone();
                                    let client3 = app_state.client.clone();
                                    let sort_val = ch.sort_order.map(|n| n.to_string()).unwrap_or_default();
                                    let ch_placeholder = ch.channel_number.map(|n| n.to_string()).unwrap_or_else(|| "–".into());
                                    let name_val = ch.custom_name.clone().unwrap_or_default();

                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                            style: if !ch.enabled { "gap:8px;padding:6px 12px;opacity:.4" } else { "gap:8px;padding:6px 12px" },

                                            input {
                                                r#type: "checkbox",
                                                checked: ch.enabled,
                                                style: "width:16px;height:16px;cursor:pointer;flex-shrink:0",
                                                onchange: {
                                                    let id = id.clone();
                                                    move |e| {
                                                        let enabled = e.value() == "true";
                                                        // optimistic update
                                                        if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                            c.enabled = enabled;
                                                        }
                                                        let c = client1.clone();
                                                        let id = id.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(PatchChannel {
                                                                id,
                                                                patch: PatchChannelRequest { enabled: Some(enabled), ..Default::default() },
                                                            }).await;
                                                        });
                                                    }
                                                },
                                            }
                                            div { class: "flex-1 min-w-0",
                                                div { style: "font-size:.82rem;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis",
                                                    "{ch.name}"
                                                }
                                                input {
                                                    class: "form-input",
                                                    style: "height:24px;font-size:.75rem;padding:2px 6px;margin-top:2px;width:100%",
                                                    value: "{name_val}",
                                                    placeholder: "Custom display name…",
                                                    onchange: {
                                                        let id = id.clone();
                                                        move |e| {
                                                            let v = e.value();
                                                            let custom = if v.is_empty() { None } else { Some(v.clone()) };
                                                            if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                                c.custom_name = custom.clone();
                                                            }
                                                            let c = client3.clone();
                                                            let id = id.clone();
                                                            spawn(async move {
                                                                let _ = c.execute(PatchChannel {
                                                                    id,
                                                                    patch: PatchChannelRequest { custom_name: custom, ..Default::default() },
                                                                }).await;
                                                            });
                                                        }
                                                    },
                                                }
                                            }
                                            input {
                                                class: "form-input",
                                                r#type: "number",
                                                style: "width:80px;height:28px;font-size:.8rem;padding:2px 6px;flex-shrink:0;text-align:right",
                                                value: "{sort_val}",
                                                placeholder: "{ch_placeholder}",
                                                onchange: {
                                                    let id = id.clone();
                                                    move |e| {
                                                        let v = e.value().parse::<i64>().ok();
                                                        if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                            c.sort_order = v;
                                                        }
                                                        let c = client2.clone();
                                                        let id = id.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(PatchChannel {
                                                                id,
                                                                patch: PatchChannelRequest { sort_order: v, ..Default::default() },
                                                            }).await;
                                                        });
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Pagination bar
                    if total_pages > 1 {
                        div { class: "pagination-bar",
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.75rem",
                                disabled: page_v == 0,
                                onclick: move |_| page.set(page_v.saturating_sub(1)),
                                "‹ Prev"
                            }
                            span { style: "font-size:.8rem;opacity:.7",
                                "Page {page_v + 1} of {total_pages}"
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.75rem",
                                disabled: page_v + 1 >= total_pages,
                                onclick: move |_| page.set(page_v + 1),
                                "Next ›"
                            }
                        }
                    }
                }
            }
        }

        // Filter / Sort modal
        if *show_filter_modal.read() {
            div { class: "modal-backdrop",
                onclick: move |_| show_filter_modal.set(false),
                div { class: "modal",
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        span { class: "modal-title", "Filters & Sort" }
                    }
                    div { class: "modal-body",
                        div { class: "form-group",
                            label { class: "form-label", "Search" }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "Search channels…",
                                value: "{search_input.read()}",
                                oninput: move |e| search_input.set(e.value()),
                                onkeydown: move |e| {
                                    if e.key() == Key::Enter {
                                        do_search();
                                        show_filter_modal.set(false);
                                    }
                                },
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Sort by" }
                            select {
                                class: "form-input",
                                value: "{sort_mode.read()}",
                                onchange: move |e| { sort_mode.set(e.value()); page.set(0); },
                                option { value: "order", "Order" }
                                option { value: "name", "Name" }
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Status" }
                            select {
                                class: "form-input",
                                value: "{enabled_filter.read()}",
                                onchange: move |e| { enabled_filter.set(e.value()); page.set(0); },
                                option { value: "all", "All" }
                                option { value: "true", "Enabled" }
                                option { value: "false", "Disabled" }
                            }
                        }
                        if !countries.read().is_empty() {
                            div { class: "form-group",
                                label { class: "form-label", "Country" }
                                select {
                                    class: "form-input",
                                    value: "{country_filter.read()}",
                                    onchange: move |e| { country_filter.set(e.value()); page.set(0); },
                                    option { value: "", "All countries" }
                                    for c in countries.read().clone() {
                                        option { value: "{c}", "{c}" }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| {
                                search_input.set(String::new());
                                search_committed.set(String::new());
                                enabled_filter.set("all".to_string());
                                country_filter.set(String::new());
                                sort_mode.set("order".to_string());
                                page.set(0);
                            },
                            "Reset"
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                do_search();
                                show_filter_modal.set(false);
                            },
                            "Done"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn UsersPage(app_state: AppState) -> Element {
    let mut users: Signal<Vec<UserDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);
    let mut form_mode: Signal<Option<UserFormMode>> = use_signal(|| None);

    // ID of the currently logged-in user (to disable self-delete)
    let self_id = app_state.server.user_id.clone();

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client.execute(GetUsers).await {
                Ok(list) => {
                    users.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load users: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Users" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| form_mode.set(Some(UserFormMode::Create)),
                    "+ New User"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if users.read().is_empty() {
                    div { class: "empty-state", "No users found" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for user in users.read().clone() {
                                {
                                    let is_self   = user.id.to_string() == self_id;
                                    let is_admin  = user.policy.is_administrator;
                                    let user_edit = user.clone();
                                    let user_id   = user.id;
                                    let client_del = app_state.client.clone();
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]", key: "{user.id}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "user-info",
                                                    span { class: "user-name", "{user.name}" }
                                                    if is_self {
                                                        span { class: "user-badge user-badge-self", "You" }
                                                    }
                                                    if is_admin {
                                                        span { class: "user-badge user-badge-admin", "Admin" }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px",
                                                    onclick: move |_| form_mode.set(Some(UserFormMode::Edit(user_edit.clone()))),
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                    disabled: is_self,
                                                    onclick: move |_| {
                                                        let c = client_del.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(DeleteUser { user_id }).await;
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                    "Delete"
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

        if let Some(mode) = form_mode.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    UserForm {
                        mode,
                        app_state: app_state.clone(),
                        on_done: move |_| {
                            form_mode.set(None);
                            let v = *refresh.peek() + 1;
                            refresh.set(v);
                        },
                        on_cancel: move |_| form_mode.set(None),
                    }
                }
            }
        }
    }
}

#[component]
fn UserForm(
    mode: UserFormMode,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let is_edit = matches!(mode, UserFormMode::Edit(_));
    let existing: Option<UserDto> = match &mode {
        UserFormMode::Edit(u) => Some(u.clone()),
        UserFormMode::Create => None,
    };

    let mut username = use_signal(|| {
        existing
            .as_ref()
            .map(|u| u.name.clone())
            .unwrap_or_default()
    });
    let mut is_admin = use_signal(|| {
        existing
            .as_ref()
            .map(|u| u.policy.is_administrator)
            .unwrap_or(false)
    });
    let mut password = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut err = use_signal(|| Option::<String>::None);
    let fr_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| u.policy.filter_rules.as_ref())
            .map(|f| f.match_mode.clone())
            .unwrap_or(FilterMatchMode::All)
    });
    let fr_rules: Signal<Vec<FilterRule>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| u.policy.filter_rules.as_ref())
            .map(|f| f.rules.clone())
            .unwrap_or_default()
    });
    let sf_stream_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| u.policy.stream_filter.as_ref())
            .map(|f| f.match_mode.clone())
            .unwrap_or(FilterMatchMode::All)
    });
    let sf_stream_rules: Signal<Vec<StreamRule>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| u.policy.stream_filter.as_ref())
            .map(|f| f.rules.clone())
            .unwrap_or_default()
    });
    let mut enable_remote_search = use_signal(|| {
        existing
            .as_ref()
            .map(|u| u.policy.enable_remote_search)
            .unwrap_or(true)
    });
    let mut max_active_sessions: Signal<i64> = use_signal(|| {
        existing
            .as_ref()
            .map(|u| u.policy.max_active_sessions)
            .unwrap_or(0)
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let pw = password.peek().clone();
        let pw2 = password2.peek().clone();
        if !pw.is_empty() && pw != pw2 {
            err.set(Some("Passwords do not match".into()));
            return;
        }
        if !is_edit && pw.is_empty() {
            err.set(Some("Password is required".into()));
            return;
        }

        let client = app_state.client.clone();
        let name = username.peek().clone();
        let admin = *is_admin.peek();
        let user_dto = existing.clone();
        let rules_snapshot = fr_rules.peek().clone();
        let match_snapshot = fr_match.peek().clone();
        let stream_rules_snapshot = sf_stream_rules.peek().clone();
        let stream_match_snapshot = sf_stream_match.peek().clone();
        let remote_search_snapshot = *enable_remote_search.peek();
        let max_sessions_snapshot = *max_active_sessions.peek();

        saving.set(true);
        err.set(None);
        spawn(async move {
            let filter_rules = if rules_snapshot.is_empty() {
                None
            } else {
                Some(CollectionFilter {
                    match_mode: match_snapshot,
                    rules: rules_snapshot,
                })
            };
            let stream_filter = if stream_rules_snapshot.is_empty() {
                None
            } else {
                Some(StreamFilter {
                    match_mode: stream_match_snapshot,
                    rules: stream_rules_snapshot,
                })
            };
            let result: Result<(), remux_sdks::ClientError> = async {
                if is_edit {
                    let user = user_dto.as_ref().unwrap();
                    // Update username
                    let mut updated = user.clone();
                    updated.name = name;
                    client
                        .execute(UpdateUser {
                            user_id: user.id,
                            dto: updated,
                        })
                        .await?;
                    // Update admin flag and filter rules
                    let mut policy = user.policy.clone();
                    policy.is_administrator = admin;
                    policy.filter_rules = filter_rules.clone();
                    policy.stream_filter = stream_filter.clone();
                    policy.enable_remote_search = remote_search_snapshot;
                    policy.max_active_sessions = max_sessions_snapshot;
                    client
                        .execute(UpdateUserPolicy {
                            user_id: user.id,
                            policy,
                        })
                        .await?;
                    // Change password only if provided
                    if !pw.is_empty() {
                        client
                            .execute(AdminSetPassword {
                                user_id: user.id,
                                new_pw: pw,
                            })
                            .await?;
                    }
                } else {
                    // Create user
                    let new_user =
                        client.execute(CreateUser { name, password: pw }).await?;
                    if admin
                        || filter_rules.is_some()
                        || stream_filter.is_some()
                        || !remote_search_snapshot
                        || max_sessions_snapshot > 0
                    {
                        let mut policy = new_user.policy.clone();
                        policy.is_administrator = admin;
                        policy.filter_rules = filter_rules.clone();
                        policy.stream_filter = stream_filter.clone();
                        policy.enable_remote_search = remote_search_snapshot;
                        policy.max_active_sessions = max_sessions_snapshot;
                        client
                            .execute(UpdateUserPolicy {
                                user_id: new_user.id,
                                policy,
                            })
                            .await?;
                    }
                }
                Ok(())
            }
            .await;

            match result {
                Ok(_) => on_done.call(()),
                Err(e) => {
                    err.set(Some(e.user_message()));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        p { class: "modal-title",
            if is_edit { "Edit User" } else { "New User" }
        }

        form {
            onsubmit: on_submit,
            style: "display:flex;flex-direction:column;gap:14px",

            div { class: "field",
                label { class: "field-label", r#for: "u-name", "Username" }
                input {
                    id: "u-name",
                    r#type: "text",
                    class: "field-input",
                    required: true,
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "u-pw",
                    if is_edit { "New Password" } else { "Password" }
                }
                input {
                    id: "u-pw",
                    r#type: "password",
                    class: "field-input",
                    required: !is_edit,
                    placeholder: if is_edit { "Leave blank to keep current" } else { "" },
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }
            }

            if !password.read().is_empty() || !is_edit {
                div { class: "field",
                    label { class: "field-label", r#for: "u-pw2", "Confirm Password" }
                    input {
                        id: "u-pw2",
                        r#type: "password",
                        class: "field-input",
                        required: !is_edit,
                        value: "{password2}",
                        oninput: move |e| password2.set(e.value()),
                    }
                }
            }

            div { class: "toggle-row",
                span { class: "toggle-label", "Administrator" }
                label { class: "toggle",
                    input {
                        r#type: "checkbox",
                        checked: *is_admin.read(),
                        onchange: move |e| is_admin.set(e.checked()),
                    }
                    span { class: "toggle-track" }
                }
            }

            div { class: "toggle-row",
                span { class: "toggle-label", "Allow Remote Search" }
                label { class: "toggle",
                    input {
                        r#type: "checkbox",
                        checked: *enable_remote_search.read(),
                        onchange: move |e| enable_remote_search.set(e.checked()),
                    }
                    span { class: "toggle-track" }
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "u-max-streams", "Max Concurrent Streams" }
                input {
                    id: "u-max-streams",
                    r#type: "number",
                    class: "field-input",
                    min: "1",
                    placeholder: "Unlimited",
                    value: if *max_active_sessions.read() > 0 { max_active_sessions.read().to_string() } else { String::new() },
                    oninput: move |e| {
                        let v = e.value();
                        max_active_sessions.set(
                            v.parse::<i64>().map(|n| n.max(1)).unwrap_or(0)
                        );
                    },
                }
                span { class: "field-hint", "Leave blank for unlimited" }
            }

            FilterRuleEditor {
                match_mode: fr_match,
                rules: fr_rules,
            }

            div { style: "margin-top:10px",
                StreamFilterEditor {
                    match_mode: sf_stream_match,
                    rules: sf_stream_rules,
                }
            }

            if let Some(e) = err.read().as_ref() {
                div { class: "alert-error", "{e}" }
            }

            div { class: "form-actions",
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
                button {
                    r#type: "submit",
                    class: "btn btn-primary",
                    disabled: *saving.read(),
                    if *saving.read() { "Saving…" } else { "Save" }
                }
            }
        }
    }
}

#[component]
fn ApiKeysPage(app_state: AppState) -> Element {
    let mut keys: Signal<Vec<AuthenticationInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Create-key dialog state
    let mut show_create = use_signal(|| false);
    let mut app_name_input = use_signal(String::new);
    let mut creating = use_signal(|| false);

    // Reveal dialog — shows the new key once after creation
    let mut revealed_key = use_signal(|| Option::<AuthenticationInfo>::None);

    // Confirm-delete state
    let mut key_to_delete: Signal<Option<String>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client.execute(GetApiKeys).await {
                Ok(result) => {
                    keys.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load API keys: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "API Keys" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        app_name_input.set(String::new());
                        show_create.set(true);
                    },
                    "+ New API Key"
                }
            }
            div { class: "card-body tight",
                p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                    "API keys allow external applications to communicate with the server without a user login."
                }
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if keys.read().is_empty() {
                    div { class: "empty-state", "No API keys — create one to get started." }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for key in keys.read().clone() {
                                {
                                    let token = key.access_token.clone().unwrap_or_default();
                                    let app = key.app_name.clone().unwrap_or_default();
                                    let created = key.date_created
                                        .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")))
                                        .unwrap_or_else(|| "—".to_string());
                                     let token_del = token.clone();
                                    rsx! {
                                        div {
                                            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                            key: "{token}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { style: "font-weight:500;font-size:.85rem", "{app}" }
                                                div { style: "font-size:.72rem;color:var(--text-muted);font-family:monospace;margin-top:2px;word-break:break-all", "{token}" }
                                                div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px", "Created: {created}" }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                    onclick: move |_| key_to_delete.set(Some(token_del.clone())),
                                                    "Revoke"
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

        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "New API Key" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.8rem;color:var(--text-muted);margin-bottom:12px",
                            "Enter a name to identify the application using this key."
                        }
                        div { class: "form-group",
                            label { class: "form-label", "App name" }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "e.g. My Media App",
                                value: "{app_name_input}",
                                oninput: move |e| app_name_input.set(e.value()),
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *creating.read() || app_name_input.read().trim().is_empty(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let name = app_name_input.read().trim().to_string();
                                    if name.is_empty() { return; }
                                    creating.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(CreateApiKey { app: name }).await {
                                            Ok(new_key) => {
                                                show_create.set(false);
                                                revealed_key.set(Some(new_key));
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to create key: {e}")));
                                                show_create.set(false);
                                            }
                                        }
                                        creating.set(false);
                                    });
                                }
                            },
                            if *creating.read() { "Creating…" } else { "Create" }
                        }
                    }
                }
            }
        }

        if let Some(new_key) = revealed_key.read().clone() {
            {
                let token = new_key.access_token.clone().unwrap_or_default();
                let app = new_key.app_name.clone().unwrap_or_default();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "API Key Created" }
                            }
                            div { class: "modal-body",
                                p { style: "font-size:.8rem;color:var(--text-muted);margin-bottom:12px",
                                    "Your new API key for "{app}" has been created. Copy it now — it will not be shown again."
                                }
                                div { class: "form-group",
                                    label { class: "form-label", "API Key" }
                                    div { style: "display:flex;gap:6px;align-items:center",
                                        input {
                                            class: "form-input",
                                            r#type: "text",
                                            readonly: true,
                                            value: "{token}",
                                            style: "font-family:monospace;font-size:.8rem",
                                        }
                                        button {
                                            class: "btn btn-ghost",
                                            style: "height:36px;white-space:nowrap;flex-shrink:0",
                                            onclick: {
                                                let t = token.clone();
                                                move |_| {
                                                    if let Some(win) = web_sys::window() {
                                                        let _ = win.navigator().clipboard().write_text(&t);
                                                    }
                                                }
                                            },
                                            "Copy"
                                        }
                                    }
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-primary",
                                    onclick: move |_| revealed_key.set(None),
                                    "Done"
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(token) = key_to_delete.read().clone() {
            {
                let client = app_state.client.clone();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "Revoke API Key" }
                            }
                            div { class: "modal-body",
                                p { style: "font-size:.85rem",
                                    "Are you sure you want to revoke this key? Any application using it will lose access immediately."
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| key_to_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-ghost",
                                    style: "color:var(--error);border-color:var(--error)",
                                    disabled: *deleting.read(),
                                    onclick: {
                                        let t = token.clone();
                                        let c = client.clone();
                                        move |_| {
                                            deleting.set(true);
                                            let tok = t.clone();
                                            let cc = c.clone();
                                            spawn(async move {
                                                let _ = cc.execute(DeleteApiKey { key: tok }).await;
                                                key_to_delete.set(None);
                                                deleting.set(false);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            });
                                        }
                                    },
                                    if *deleting.read() { "Revoking…" } else { "Revoke" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingsPage(app_state: AppState) -> Element {
    rsx! {
        ServerSettingsCard { app_state: app_state.clone() }
        PlaybackSettingsCard { app_state: app_state.clone() }
        SearchSettingsCard { app_state: app_state.clone() }
        JellyfinImportCard { app_state }
    }
}

#[component]
fn ServerSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut server_name = use_signal(String::new);
    let mut metadata_country = use_signal(|| "US".to_string());
    let mut countries: Signal<Vec<CountryInfo>> = use_signal(Vec::new);
    let mut catalog_max_items = use_signal(|| 100_i64);
    let mut filter_digital_release = use_signal(|| true);
    let mut digital_release_buffer = use_signal(|| 0_i64);
    let mut subtitle_languages = use_signal(String::new);
    let mut quick_connect_enabled = use_signal(|| true);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    server_name.set(cfg.server_name.clone().unwrap_or_default());
                    metadata_country.set(
                        cfg.metadata_country_code
                            .clone()
                            .unwrap_or_else(|| "US".to_string()),
                    );
                    catalog_max_items.set(cfg.catalog_max_items.unwrap_or(100));
                    filter_digital_release.set(cfg.filter_by_digital_release_date);
                    digital_release_buffer.set(cfg.digital_release_buffer_days);
                    subtitle_languages.set(
                        cfg.subtitle_languages
                            .as_deref()
                            .map(|v| v.join(", "))
                            .unwrap_or_default(),
                    );
                    quick_connect_enabled
                        .set(cfg.quick_connect_available.unwrap_or(true));
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            if let Ok(list) = client.execute(GetCountries).await {
                countries.set(list);
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let name = server_name.peek().clone();
        let country = metadata_country.peek().clone();
        let max = *catalog_max_items.peek();
        let filter_dr = *filter_digital_release.peek();
        let dr_buffer = *digital_release_buffer.peek();
        let sub_langs_str = subtitle_languages.peek().clone();
        let qc_enabled = *quick_connect_enabled.peek();

        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        cfg.server_name = Some(name);
        cfg.metadata_country_code = Some(country);
        cfg.quick_connect_available = Some(qc_enabled);
        cfg.catalog_max_items = Some(max);
        cfg.filter_by_digital_release_date = filter_dr;
        cfg.digital_release_buffer_days = dr_buffer;
        cfg.subtitle_languages = Some(
            sub_langs_str
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
        );

        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "General Settings" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form {
                        onsubmit: on_submit,
                        style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "s-name", "Server Name" }
                            input {
                                id: "s-name",
                                r#type: "text",
                                class: "field-input",
                                value: "{server_name}",
                                oninput: move |e| server_name.set(e.value()),
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-country", "Metadata Country" }
                            select {
                                id: "s-country",
                                class: "select-input",
                                value: "{metadata_country}",
                                onchange: move |e| metadata_country.set(e.value()),
                                for country in countries.read().iter() {
                                    option {
                                        value: "{country.two_letter_iso_region_name}",
                                        selected: metadata_country.read().as_str() == country.two_letter_iso_region_name,
                                        "{country.name} ({country.two_letter_iso_region_name})"
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-max", "Catalog Max Items" }
                            input {
                                id: "s-max",
                                r#type: "number",
                                class: "field-input",
                                min: "1",
                                value: "{catalog_max_items}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        catalog_max_items.set(n);
                                    }
                                },
                            }
                            p { class: "field-hint",
                                "Maximum number of items imported per collection."
                            }
                        }

                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *filter_digital_release.read(),
                                    oninput: move |e| filter_digital_release.set(e.checked()),
                                }
                                " Filter by digital release date"
                            }
                            p { class: "field-hint",
                                "Hide items that haven't been digitally released yet. Falls back to theatrical release date when no digital date is set."
                            }
                        }

                        if *filter_digital_release.read() {
                            div { class: "field",
                                label { class: "field-label", r#for: "s-dr-buf", "Release buffer (days)" }
                                input {
                                    id: "s-dr-buf",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    max: "365",
                                    value: "{digital_release_buffer}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() {
                                            digital_release_buffer.set(n);
                                        }
                                    },
                                }
                                p { class: "field-hint",
                                    "Show items releasing up to this many days in the future. 0 = today or earlier only."
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-sub-langs", "Subtitle Languages" }
                            input {
                                id: "s-sub-langs",
                                r#type: "text",
                                class: "field-input",
                                placeholder: "en, de, fr",
                                value: "{subtitle_languages}",
                                oninput: move |e| subtitle_languages.set(e.value()),
                            }
                            p { class: "field-hint",
                                "Comma-separated ISO 639-1 codes (e.g. \"en, de\"). "
                                "Only subtitles in these languages are shown and the first match is selected by default. "
                                "Leave empty to show all subtitles without a default."
                            }
                        }

                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *quick_connect_enabled.read(),
                                    oninput: move |e| quick_connect_enabled.set(e.checked()),
                                }
                                " Enable QuickConnect"
                            }
                            p { class: "field-hint",
                                "Allow clients to log in by entering a code shown on the login screen."
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Settings saved." }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlaybackSettingsCard(app_state: AppState) -> Element {
    let mut encoding_preset = use_signal(|| "fast".to_string());
    let mut hw_accel = use_signal(|| "none".to_string());
    let mut auto_detect = use_signal(|| true);
    let mut enable_tonemapping = use_signal(|| false);
    let mut enable_vpp_tonemapping = use_signal(|| false);
    let mut tonemapping_algorithm = use_signal(|| "hable".to_string());
    let mut tonemapping_desat = use_signal(|| 0.0_f32);
    let mut tonemapping_peak = use_signal(|| 0.0_f32);
    let mut allow_hevc_encoding = use_signal(|| false);
    let mut allow_av1_encoding = use_signal(|| false);
    let mut h264_crf = use_signal(|| 23_u32);
    let mut h265_crf = use_signal(|| 28_u32);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetEncodingConfiguration).await {
                Ok(opts) => {
                    encoding_preset.set(
                        opts.encoding_preset.unwrap_or_else(|| "fast".to_string()),
                    );
                    auto_detect
                        .set(opts.auto_detect_hardware_acceleration.unwrap_or(true));
                    let accel_str =
                        match opts.hardware_acceleration_type.unwrap_or_default() {
                            HardwareAccelerationType::None => "none",
                            HardwareAccelerationType::Vaapi => "vaapi",
                            HardwareAccelerationType::Nvenc => "nvenc",
                            HardwareAccelerationType::Qsv => "qsv",
                            HardwareAccelerationType::Amf => "amf",
                            HardwareAccelerationType::VideoToolbox => "videotoolbox",
                            HardwareAccelerationType::V4l2m2m => "v4l2m2m",
                            HardwareAccelerationType::Rkmpp => "rkmpp",
                        };
                    hw_accel.set(accel_str.to_string());
                    enable_tonemapping.set(opts.enable_tonemapping.unwrap_or(false));
                    enable_vpp_tonemapping
                        .set(opts.enable_vpp_tonemapping.unwrap_or(false));
                    tonemapping_algorithm.set(
                        opts.tonemapping_algorithm
                            .unwrap_or_else(|| "hable".to_string()),
                    );
                    tonemapping_desat.set(opts.tonemapping_desat.unwrap_or(0.0));
                    tonemapping_peak.set(opts.tonemapping_peak.unwrap_or(0.0));
                    allow_hevc_encoding.set(opts.allow_hevc_encoding.unwrap_or(true));
                    allow_av1_encoding.set(opts.allow_av1_encoding.unwrap_or(false));
                    h264_crf.set(opts.h264_crf.unwrap_or(23));
                    h265_crf.set(opts.h265_crf.unwrap_or(28));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let accel_type = match hw_accel.peek().as_str() {
            "vaapi" => HardwareAccelerationType::Vaapi,
            "nvenc" => HardwareAccelerationType::Nvenc,
            "qsv" => HardwareAccelerationType::Qsv,
            "amf" => HardwareAccelerationType::Amf,
            "videotoolbox" => HardwareAccelerationType::VideoToolbox,
            "v4l2m2m" => HardwareAccelerationType::V4l2m2m,
            "rkmpp" => HardwareAccelerationType::Rkmpp,
            _ => HardwareAccelerationType::None,
        };
        let opts = EncodingOptions {
            encoding_preset: Some(encoding_preset.peek().clone()),
            hardware_acceleration_type: Some(accel_type),
            vaapi_device: None,
            vaapi_driver: None,
            auto_detect_hardware_acceleration: Some(*auto_detect.peek()),
            enable_tonemapping: Some(*enable_tonemapping.peek()),
            enable_vpp_tonemapping: Some(*enable_vpp_tonemapping.peek()),
            tonemapping_algorithm: Some(tonemapping_algorithm.peek().clone()),
            tonemapping_desat: Some(*tonemapping_desat.peek()),
            tonemapping_peak: Some(*tonemapping_peak.peek()),
            allow_hevc_encoding: Some(*allow_hevc_encoding.peek()),
            allow_av1_encoding: Some(*allow_av1_encoding.peek()),
            h264_crf: Some(*h264_crf.peek()),
            h265_crf: Some(*h265_crf.peek()),
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateEncodingConfiguration { config: opts })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Playback" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label", "Hardware Acceleration" }
                            div { class: "field-hint",
                                "GPU-accelerated video encoding. When auto-detect is on, the server probes available hardware at startup and selects the best option."
                            }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *auto_detect.read(),
                                    onchange: move |e| auto_detect.set(e.checked()),
                                }
                                "Auto-detect at startup"
                            }
                            select {
                                id: "hw-accel",
                                class: "select-input",
                                disabled: *auto_detect.read(),
                                value: "{hw_accel}",
                                onchange: move |e| hw_accel.set(e.value()),
                                option { value: "none", "None (Software)" }
                                option { value: "vaapi", "VAAPI (Intel/AMD on Linux)" }
                                option { value: "nvenc", "NVENC (NVIDIA)" }
                                option { value: "qsv", "Quick Sync (Intel)" }
                                option { value: "amf", "AMF (AMD on Windows)" }
                                option { value: "videotoolbox", "VideoToolBox (macOS/Apple)" }
                                option { value: "v4l2m2m", "V4L2M2M (ARM/embedded)" }
                                option { value: "rkmpp", "RKMPP (Rockchip)" }
                            }
                            if *auto_detect.read() {
                                div { class: "field-hint", style: "margin-top:6px",
                                    "Currently using: {hw_accel} (detected at last startup)"
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "encoding-preset", "Encoding Preset" }
                            div { class: "field-hint", "FFmpeg -preset for software transcoding. Faster presets use more CPU; slower presets produce smaller files." }
                            select {
                                id: "encoding-preset",
                                class: "select-input",
                                value: "{encoding_preset}",
                                onchange: move |e| encoding_preset.set(e.value()),
                                option { value: "ultrafast", "Ultra Fast" }
                                option { value: "superfast", "Super Fast" }
                                option { value: "veryfast", "Very Fast" }
                                option { value: "faster", "Faster" }
                                option { value: "fast", "Fast (default)" }
                                option { value: "medium", "Medium" }
                                option { value: "slow", "Slow" }
                                option { value: "slower", "Slower" }
                                option { value: "veryslow", "Very Slow" }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Codec Gates" }
                            div { class: "field-hint", "Allow these codecs for hardware/software encoding." }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                                input {
                                    r#type: "checkbox",
                                    checked: *allow_hevc_encoding.read(),
                                    onchange: move |e| allow_hevc_encoding.set(e.checked()),
                                }
                                "Allow HEVC (H.265) encoding"
                            }
                            label { style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *allow_av1_encoding.read(),
                                    onchange: move |e| allow_av1_encoding.set(e.checked()),
                                }
                                "Allow AV1 encoding"
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Software Encoding Quality (CRF)" }
                            div { class: "field-hint", "Constant Rate Factor for libx264/libx265. Lower = better quality, larger file. Ignored when using hardware encoding or bitrate-limited streams." }
                            div { style: "display:flex;gap:16px;flex-wrap:wrap",
                                div { style: "display:flex;flex-direction:column;gap:4px",
                                    label { r#for: "h264-crf", style: "font-size:0.85em", "H.264 CRF (0–51, default 23)" }
                                    input {
                                        id: "h264-crf",
                                        r#type: "number",
                                        class: "text-input",
                                        style: "width:80px",
                                        min: "0",
                                        max: "51",
                                        value: "{h264_crf}",
                                        onchange: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                h264_crf.set(v.min(51));
                                            }
                                        },
                                    }
                                }
                                div { style: "display:flex;flex-direction:column;gap:4px",
                                    label { r#for: "h265-crf", style: "font-size:0.85em", "H.265 CRF (0–51, default 28)" }
                                    input {
                                        id: "h265-crf",
                                        r#type: "number",
                                        class: "text-input",
                                        style: "width:80px",
                                        min: "0",
                                        max: "51",
                                        value: "{h265_crf}",
                                        onchange: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                h265_crf.set(v.min(51));
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "HDR Tone Mapping" }
                            div { class: "field-hint", "Convert HDR content to SDR using tone mapping. Without tone mapping, colour metadata is rewritten so clients treat the stream as SDR (may look washed out on some content)." }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                                input {
                                    r#type: "checkbox",
                                    checked: *enable_tonemapping.read(),
                                    onchange: move |e| enable_tonemapping.set(e.checked()),
                                }
                                "Software tone mapping (tonemapx, CPU)"
                            }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *enable_vpp_tonemapping.read(),
                                    onchange: move |e| enable_vpp_tonemapping.set(e.checked()),
                                }
                                "Hardware VPP tone mapping (tonemap_vaapi, Intel VAAPI/QSV)"
                            }
                            if *enable_tonemapping.read() && !*enable_vpp_tonemapping.read() {
                                div { style: "margin-top:4px",
                                    label { class: "field-label", r#for: "tonemap-algo", style: "font-size:0.85em", "Algorithm" }
                                    select {
                                        id: "tonemap-algo",
                                        class: "select-input",
                                        style: "margin-top:4px",
                                        value: "{tonemapping_algorithm}",
                                        onchange: move |e| tonemapping_algorithm.set(e.value()),
                                        option { value: "hable", "Hable (Filmic, default)" }
                                        option { value: "reinhard", "Reinhard" }
                                        option { value: "mobius", "Mobius" }
                                        option { value: "bt2390", "BT.2390 (perceptual quantizer)" }
                                        option { value: "bt2446a", "BT.2446a" }
                                        option { value: "none", "None (clip)" }
                                    }
                                    div { style: "display:flex;gap:16px;flex-wrap:wrap;margin-top:8px",
                                        div { style: "display:flex;flex-direction:column;gap:4px",
                                            label { r#for: "tonemap-desat", style: "font-size:0.85em", "Desaturation (0 = disabled)" }
                                            input {
                                                id: "tonemap-desat",
                                                r#type: "number",
                                                class: "text-input",
                                                style: "width:80px",
                                                min: "0",
                                                max: "1",
                                                step: "0.1",
                                                value: "{tonemapping_desat}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<f32>() {
                                                        tonemapping_desat.set(v);
                                                    }
                                                },
                                            }
                                        }
                                        div { style: "display:flex;flex-direction:column;gap:4px",
                                            label { r#for: "tonemap-peak", style: "font-size:0.85em", "Peak luminance nits (0 = auto)" }
                                            input {
                                                id: "tonemap-peak",
                                                r#type: "number",
                                                class: "text-input",
                                                style: "width:90px",
                                                min: "0",
                                                step: "100",
                                                value: "{tonemapping_peak}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<f32>() {
                                                        tonemapping_peak.set(v);
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Settings saved. Restart the server to apply hardware acceleration changes." }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProbeSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut probe_timeout = use_signal(|| 20_i64);
    let mut probe_timeout_p2p = use_signal(|| 60_i64);
    let mut auto_next_stream = use_signal(|| true);
    let mut max_fallback_streams = use_signal(|| 3_i64);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    probe_timeout.set(cfg.probe_timeout_secs.unwrap_or(20));
                    probe_timeout_p2p.set(cfg.probe_timeout_p2p_secs.unwrap_or(60));
                    auto_next_stream
                        .set(cfg.auto_next_stream_on_probe_fail.unwrap_or(true));
                    max_fallback_streams
                        .set(cfg.max_probe_fallback_streams.unwrap_or(3));
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let Some(cfg) = base_cfg.peek().clone() else {
            return;
        };
        let updated = ServerConfiguration {
            probe_timeout_secs: Some(*probe_timeout.peek()),
            probe_timeout_p2p_secs: Some(*probe_timeout_p2p.peek()),
            auto_next_stream_on_probe_fail: Some(*auto_next_stream.peek()),
            max_probe_fallback_streams: Some(*max_fallback_streams.peek()),
            ..cfg
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: updated })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Stream Probing" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label", r#for: "probe-timeout", "Probe Timeout (seconds)" }
                            div { class: "field-hint",
                                "Seconds to wait for stream probe before giving up on HTTP/local streams."
                            }
                            input {
                                id: "probe-timeout",
                                r#type: "number",
                                class: "text-input",
                                min: "1",
                                max: "300",
                                value: "{probe_timeout}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        probe_timeout.set(v);
                                    }
                                },
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "probe-timeout-p2p", "P2P Probe Timeout (seconds)" }
                            div { class: "field-hint",
                                "Seconds to wait for stream probe before giving up on torrent/P2P streams."
                            }
                            input {
                                id: "probe-timeout-p2p",
                                r#type: "number",
                                class: "text-input",
                                min: "1",
                                max: "600",
                                value: "{probe_timeout_p2p}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        probe_timeout_p2p.set(v);
                                    }
                                },
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Auto Next Stream on Probe Fail" }
                            div { class: "field-hint",
                                "When a stream probe fails, automatically try the next stream with matching resolution and type."
                            }
                            label { style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *auto_next_stream.read(),
                                    onchange: move |e| auto_next_stream.set(e.checked()),
                                }
                                "Enabled"
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "max-fallback", "Max Stream Retries" }
                            div { class: "field-hint",
                                "How many alternative streams to try before giving up and returning an error."
                            }
                            input {
                                id: "max-fallback",
                                r#type: "number",
                                class: "text-input",
                                min: "0",
                                max: "20",
                                value: "{max_fallback_streams}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        max_fallback_streams.set(v);
                                    }
                                },
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Settings saved." }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SearchSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut movies_remote = use_signal(|| true);
    let mut series_remote = use_signal(|| true);
    let mut tracks_remote = use_signal(|| true);
    let mut albums_remote = use_signal(|| true);
    let mut artists_remote = use_signal(|| true);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    let enabled = &cfg.search_remote_enabled;
                    let all = enabled.is_none();
                    let list = enabled.as_deref().unwrap_or(&[]);
                    movies_remote.set(all || list.contains(&"movie".to_string()));
                    series_remote.set(all || list.contains(&"series".to_string()));
                    tracks_remote.set(all || list.contains(&"track".to_string()));
                    albums_remote.set(all || list.contains(&"album".to_string()));
                    artists_remote.set(all || list.contains(&"artist".to_string()));
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        let mut remote_enabled: Vec<String> = vec!["person".to_string()];
        if *movies_remote.peek() {
            remote_enabled.push("movie".to_string());
        }
        if *series_remote.peek() {
            remote_enabled.push("series".to_string());
        }
        if *tracks_remote.peek() {
            remote_enabled.push("track".to_string());
        }
        if *albums_remote.peek() {
            remote_enabled.push("album".to_string());
        }
        if *artists_remote.peek() {
            remote_enabled.push("artist".to_string());
        }
        cfg.search_remote_enabled = Some(remote_enabled);
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Remote Search" }
            }
            div { class: "card-body",
            if *loading.read() {
                span { class: "loading-text", "Loading…" }
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                    div { class: "field",
                        div { class: "toggle-row",
                            span { class: "toggle-label", "Movies" }
                            label { class: "toggle",
                                input {
                                    r#type: "checkbox",
                                    checked: *movies_remote.read(),
                                    oninput: move |e| movies_remote.set(e.checked()),
                                }
                                span { class: "toggle-track" }
                            }
                        }
                    }
                    div { class: "form-field",
                        div { class: "toggle-row",
                            span { class: "toggle-label", "Series" }
                            label { class: "toggle",
                                input {
                                    r#type: "checkbox",
                                    checked: *series_remote.read(),
                                    oninput: move |e| series_remote.set(e.checked()),
                                }
                                span { class: "toggle-track" }
                            }
                        }
                    }
                    div { class: "form-field",
                        div { class: "toggle-row",
                            span { class: "toggle-label", "Tracks" }
                            label { class: "toggle",
                                input {
                                    r#type: "checkbox",
                                    checked: *tracks_remote.read(),
                                    oninput: move |e| tracks_remote.set(e.checked()),
                                }
                                span { class: "toggle-track" }
                            }
                        }
                    }
                    div { class: "form-field",
                        div { class: "toggle-row",
                            span { class: "toggle-label", "Albums" }
                            label { class: "toggle",
                                input {
                                    r#type: "checkbox",
                                    checked: *albums_remote.read(),
                                    oninput: move |e| albums_remote.set(e.checked()),
                                }
                                span { class: "toggle-track" }
                            }
                        }
                    }
                    div { class: "form-field",
                        div { class: "toggle-row",
                            span { class: "toggle-label", "Artists" }
                            label { class: "toggle",
                                input {
                                    r#type: "checkbox",
                                    checked: *artists_remote.read(),
                                    oninput: move |e| artists_remote.set(e.checked()),
                                }
                                span { class: "toggle-track" }
                            }
                        }
                    }

                    if let Some(err) = error.read().as_ref() {
                        div { class: "alert-error", "{err}" }
                    }
                    if *saved.read() {
                        div { class: "alert-success", "Settings saved." }
                    }

                    div { class: "form-actions",
                        button {
                            r#type: "submit",
                            class: "btn btn-primary",
                            disabled: *saving.read(),
                            if *saving.read() { "Saving…" } else { "Save Settings" }
                        }
                    }
                }
            }
            }
        }
    }
}

#[component]
fn JellyfinImportCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut jellyfin_url = use_signal(String::new);
    let mut jellyfin_api_key = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut save_error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);
    let mut importing = use_signal(|| false);
    let mut import_error = use_signal(|| Option::<String>::None);
    let mut import_done = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    jellyfin_url.set(cfg.jellyfin_url.clone().unwrap_or_default());
                    jellyfin_api_key
                        .set(cfg.jellyfin_api_key.clone().unwrap_or_default());
                    base_cfg.set(Some(cfg));
                }
                Err(e) => save_error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let app_state_save = app_state.clone();
    let on_save = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state_save.client.clone();
        let url = jellyfin_url.peek().clone();
        let key = jellyfin_api_key.peek().clone();

        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        cfg.jellyfin_url = if url.is_empty() { None } else { Some(url) };
        cfg.jellyfin_api_key = if key.is_empty() { None } else { Some(key) };

        saving.set(true);
        save_error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => save_error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    let on_import = move |_| {
        let client = app_state.client.clone();
        importing.set(true);
        import_error.set(None);
        import_done.set(false);
        spawn(async move {
            match client
                .execute(StartTask {
                    task_id: "JellyfinImport".into(),
                })
                .await
            {
                Ok(_) => import_done.set(true),
                Err(e) => import_error.set(Some(e.user_message())),
            }
            importing.set(false);
        });
    };

    let url_filled = !jellyfin_url.read().is_empty();
    let key_filled = !jellyfin_api_key.read().is_empty();
    let can_import = url_filled && key_filled && !*importing.read();

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Jellyfin Import" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form {
                        onsubmit: on_save,
                        style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "jf-url", "Jellyfin URL" }
                            input {
                                id: "jf-url",
                                r#type: "url",
                                class: "field-input",
                                placeholder: "http://192.168.1.x:8096",
                                value: "{jellyfin_url}",
                                oninput: move |e| jellyfin_url.set(e.value()),
                            }
                            p { class: "field-hint", "Base URL of the source Jellyfin server." }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "jf-key", "API Key" }
                            input {
                                id: "jf-key",
                                r#type: "password",
                                class: "field-input",
                                placeholder: "••••••••••••••••",
                                value: "{jellyfin_api_key}",
                                oninput: move |e| jellyfin_api_key.set(e.value()),
                            }
                            p { class: "field-hint",
                                "Found in Jellyfin → Dashboard → API Keys."
                            }
                        }

                        if let Some(err) = save_error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Settings saved." }
                        }

                        div { class: "form-actions", style: "display:flex;gap:8px;align-items:center",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save" }
                            }
                            button {
                                r#type: "button",
                                class: "btn btn-secondary",
                                disabled: !can_import,
                                onclick: on_import,
                                if *importing.read() { "Starting…" } else { "Import Users" }
                            }
                        }

                        if let Some(err) = import_error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *import_done.read() {
                            div { class: "alert-success",
                                "Import started. Check the Tasks page for progress."
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BrandingPage(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<BrandingOptions>> = use_signal(|| None);
    let mut custom_css = use_signal(String::new);
    let mut login_disclaimer = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetBrandingConfiguration).await {
                Ok(cfg) => {
                    custom_css.set(cfg.custom_css.clone().unwrap_or_default());
                    login_disclaimer
                        .set(cfg.login_disclaimer.clone().unwrap_or_default());
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load branding: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let css = custom_css.peek().clone();
        let disc = login_disclaimer.peek().clone();

        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        cfg.custom_css = if css.is_empty() { None } else { Some(css) };
        cfg.login_disclaimer = if disc.is_empty() { None } else { Some(disc) };

        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateBrandingConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Branding" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form {
                        onsubmit: on_submit,
                        style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "b-css", "Custom CSS" }
                            p { class: "field-hint", "Injected into every page of the Jellyfin web client." }
                            textarea {
                                id: "b-css",
                                class: "field-input",
                                style: "min-height:220px;resize:vertical;font-family:var(--font-mono);font-size:.78rem",
                                value: "{custom_css}",
                                oninput: move |e| custom_css.set(e.value()),
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "b-disc", "Login Disclaimer" }
                            p { class: "field-hint", "Text shown below the login form." }
                            textarea {
                                id: "b-disc",
                                class: "field-input",
                                style: "min-height:80px;resize:vertical",
                                value: "{login_disclaimer}",
                                oninput: move |e| login_disclaimer.set(e.value()),
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Branding saved." }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn WizardStep(n: u8, label: &'static str, active: bool, done: bool) -> Element {
    let dot_class = if done {
        "wizard-step-dot wizard-step-done"
    } else if active {
        "wizard-step-dot wizard-step-active"
    } else {
        "wizard-step-dot"
    };
    rsx! {
        div { class: "wizard-step",
            div { class: "{dot_class}",
                if done { "✓" } else { "{n}" }
            }
            span { class: "wizard-step-label", "{label}" }
        }
    }
}

#[component]
fn Wizard(on_complete: EventHandler) -> Element {
    let mut step = use_signal(|| 0_u8);
    let mut server_name = use_signal(String::new);
    let mut metadata_country = use_signal(browser_metadata_country_code);
    let mut countries: Signal<Vec<CountryInfo>> = use_signal(Vec::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);

    // Pre-fill from current startup config (in case the wizard was partially run)
    use_effect(move || {
        let origin = get_origin();
        spawn(async move {
            if let Ok(c) = remux_sdks::remux::client(&origin) {
                if let Ok(cfg) = c.execute(GetStartupConfiguration::default()).await {
                    if let Some(name) = cfg.server_name.filter(|s| !s.is_empty()) {
                        server_name.set(name);
                    }
                    metadata_country.set(
                        cfg.metadata_country_code
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(browser_metadata_country_code),
                    );
                }
                if let Ok(list) = c.execute(GetCountries).await {
                    countries.set(list);
                }
            }
        });
    });

    rsx! {
        div { class: "wizard-page",
            div { class: "wizard-card",

                div { class: "wizard-steps",
                    WizardStep { n: 1, label: "Server",  active: *step.read() == 0, done: *step.read() > 0 }
                    div { class: "wizard-step-line" }
                    WizardStep { n: 2, label: "Account", active: *step.read() == 1, done: *step.read() > 1 }
                    div { class: "wizard-step-line" }
                    WizardStep { n: 3, label: "Done",    active: *step.read() == 2, done: false }
                }

                div { class: "wizard-header",
                    span { class: "login-brand-label", "Remux" }
                    h2 { class: "wizard-title",
                        {match *step.read() {
                            0 => "Server Configuration",
                            1 => "Create Admin Account",
                            _ => "Setup Complete",
                        }}
                    }
                }

                div { class: "wizard-body",
                    if let Some(err) = error.read().as_ref() {
                        div { class: "alert-error", style: "margin-bottom:16px", "{err}" }
                    }

                    {match *step.read() {

                        0 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = server_name.peek().clone();
                                    let country = metadata_country.peek().clone();
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match remux_sdks::remux::client(&origin) {
                                            Ok(c) => match c.execute(PostStartupConfiguration {
                                                payload: StartupConfiguration {
                                                    server_name: Some(name),
                                                    metadata_country_code: Some(country),
                                                    ..Default::default()
                                                },
                                            }).await {
                                                Ok(_)  => step.set(1),
                                                Err(e) => error.set(Some(format!("{e}"))),
                                            },
                                            Err(e) => error.set(Some(format!("Client error: {e}"))),
                                        }
                                        saving.set(false);
                                    });
                                },
                                style: "display:flex;flex-direction:column;gap:16px",

                                p { class: "wizard-desc",
                                    "Give your server a name. Add media addons (Stremio, Deezer, TMDB, …) on the Addons page after setup."
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-name", "Server Name" }
                                    input {
                                        id: "w-name",
                                        r#type: "text",
                                        class: "field-input",
                                        placeholder: "My Remux Server",
                                        value: "{server_name}",
                                        oninput: move |e| server_name.set(e.value()),
                                    }
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-country", "Metadata Country" }
                                    select {
                                        id: "w-country",
                                        class: "select-input",
                                        value: "{metadata_country}",
                                        onchange: move |e| metadata_country.set(e.value()),
                                        if countries.read().is_empty() {
                                            option {
                                                value: "{metadata_country}",
                                                selected: true,
                                                "{metadata_country}"
                                            }
                                        }
                                        for country in countries.read().iter() {
                                            option {
                                                value: "{country.two_letter_iso_region_name}",
                                                selected: metadata_country.read().as_str() == country.two_letter_iso_region_name,
                                                "{country.name} ({country.two_letter_iso_region_name})"
                                            }
                                        }
                                    }
                                    p { class: "field-hint",
                                        "Used for metadata ratings and regional release details."
                                    }
                                }

                                div { class: "wizard-actions",
                                    button {
                                        r#type: "submit",
                                        class: "btn btn-primary",
                                        disabled: *saving.read(),
                                        if *saving.read() { "Saving…" } else { "Next →" }
                                    }
                                }
                            }
                        },

                        1 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = username.peek().clone();
                                    let pw   = password.peek().clone();
                                    let pw2  = password2.peek().clone();
                                    let name = match Username::try_new(name) {
                                        Ok(u) => u,
                                        Err(_) => {
                                            error.set(Some("Invalid username: must contain only letters, digits, spaces, and -'._@+, and be at most 255 characters".into()));
                                            return;
                                        }
                                    };
                                    if pw != pw2 {
                                        error.set(Some("Passwords do not match".into()));
                                        return;
                                    }
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match remux_sdks::remux::client(&origin) {
                                            Ok(c) => match c.execute(PostStartupUser {
                                                payload: StartupUser {
                                                    name: Some(name),
                                                    password: Some(pw.clone()),
                                                    password_confirm: Some(pw),
                                                },
                                            }).await {
                                                Ok(_)  => step.set(2),
                                                Err(e) => error.set(Some(format!("{e}"))),
                                            },
                                            Err(e) => error.set(Some(format!("Client error: {e}"))),
                                        }
                                        saving.set(false);
                                    });
                                },
                                style: "display:flex;flex-direction:column;gap:16px",

                                p { class: "wizard-desc",
                                    "Create the administrator account you will use to log in."
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-user", "Username" }
                                    input {
                                        id: "w-user",
                                        r#type: "text",
                                        class: "field-input",
                                        required: true,
                                        value: "{username}",
                                        oninput: move |e| username.set(e.value()),
                                        autocomplete: "username",
                                    }
                                }
                                div { class: "field",
                                    label { class: "field-label", r#for: "w-pw", "Password" }
                                    input {
                                        id: "w-pw",
                                        r#type: "password",
                                        class: "field-input",
                                        required: true,
                                        value: "{password}",
                                        oninput: move |e| password.set(e.value()),
                                        autocomplete: "new-password",
                                    }
                                }
                                div { class: "field",
                                    label { class: "field-label", r#for: "w-pw2", "Confirm Password" }
                                    input {
                                        id: "w-pw2",
                                        r#type: "password",
                                        class: "field-input",
                                        required: true,
                                        value: "{password2}",
                                        oninput: move |e| password2.set(e.value()),
                                        autocomplete: "new-password",
                                    }
                                }

                                div { class: "wizard-actions wizard-actions-split",
                                    button {
                                        r#type: "button",
                                        class: "btn btn-ghost",
                                        onclick: move |_| { error.set(None); step.set(0); },
                                        "← Back"
                                    }
                                    button {
                                        r#type: "submit",
                                        class: "btn btn-primary",
                                        disabled: *saving.read(),
                                        if *saving.read() { "Creating…" } else { "Next →" }
                                    }
                                }
                            }
                        },

                        _ => rsx! {
                            div { style: "display:flex;flex-direction:column;gap:20px",
                                p { class: "wizard-desc",
                                    "Your server is configured and the admin account has been created. "
                                    "Click Finish to complete setup and go to the login page."
                                }
                                div { class: "wizard-actions",
                                    button {
                                        class: "btn btn-primary",
                                        style: "width:100%",
                                        disabled: *saving.read(),
                                        onclick: move |_| {
                                            let origin = get_origin();
                                            saving.set(true);
                                            error.set(None);
                                            spawn(async move {
                                                if let Ok(c) = remux_sdks::remux::client(&origin) {
                                                    let _ = c.execute(PostStartupComplete::default()).await;
                                                }
                                                on_complete.call(());
                                            });
                                        },
                                        if *saving.read() { "Finishing…" } else { "Finish Setup" }
                                    }
                                }
                            }
                        },
                    }}
                }
            }
        }
    }
}

#[component]
fn P2pSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut p2p_enabled = use_signal(|| true);
    let mut p2p_upload_speed = use_signal(|| 0_i64);
    let mut p2p_download_speed = use_signal(|| 0_i64);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    p2p_enabled.set(cfg.p2p_enabled.unwrap_or(true));
                    p2p_upload_speed.set(cfg.p2p_upload_speed_kbps.unwrap_or(0));
                    p2p_download_speed.set(cfg.p2p_download_speed_kbps.unwrap_or(0));
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let Some(cfg) = base_cfg.peek().clone() else {
            return;
        };
        let updated = ServerConfiguration {
            p2p_enabled: Some(*p2p_enabled.peek()),
            p2p_upload_speed_kbps: Some(*p2p_upload_speed.peek()),
            p2p_download_speed_kbps: Some(*p2p_download_speed.peek()),
            ..cfg
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: updated })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "P2P / Torrent Streams" }
            }
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else {
                    form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *p2p_enabled.read(),
                                    oninput: move |e| p2p_enabled.set(e.checked()),
                                }
                                " Enable P2P Streams"
                            }
                            p { class: "field-hint", "Allow torrent/magnet streams from AIO sources." }
                        }

                        if *p2p_enabled.read() {
                            div { class: "field",
                                label { class: "field-label", r#for: "p2p-up", "Upload Speed Limit (KB/s)" }
                                input {
                                    id: "p2p-up",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_upload_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() { p2p_upload_speed.set(n); }
                                    },
                                }
                                p { class: "field-hint", "0 = no uploading (seeding disabled)." }
                            }

                            div { class: "field",
                                label { class: "field-label", r#for: "p2p-down", "Download Speed Limit (KB/s)" }
                                input {
                                    id: "p2p-down",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_download_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() { p2p_download_speed.set(n); }
                                    },
                                }
                                p { class: "field-hint", "0 = unlimited." }
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }
                        if *saved.read() {
                            div { class: "alert-success", "Settings saved." }
                        }
                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StreamsPage(app_state: AppState) -> Element {
    rsx! {
        ProbeSettingsCard { app_state: app_state.clone() }
        P2pSettingsCard { app_state: app_state.clone() }
        StreamGroupsCard { app_state }
    }
}

#[component]
fn StreamRuleRow(
    idx: usize,
    rule: StreamRule,
    rules: Signal<Vec<StreamRule>>,
) -> Element {
    let field_val = match &rule {
        StreamRule::Resolution { .. } => "resolution",
        StreamRule::Quality { .. } => "quality",
        StreamRule::Codec { .. } => "codec",
    };
    let op_not_in = match &rule {
        StreamRule::Resolution { op, .. }
        | StreamRule::Quality { op, .. }
        | StreamRule::Codec { op, .. } => matches!(op, SetOp::NotIn),
    };

    rsx! {
        div { style: "display:flex;align-items:flex-start;gap:6px",
            // Field selector
            select {
                class: "select-input",
                style: "flex:1.2",
                value: "{field_val}",
                onchange: move |e| {
                    if let Some(r) = rules.write().get_mut(idx) {
                        *r = match e.value().as_str() {
                            "quality" => StreamRule::Quality { op: SetOp::In, values: vec![] },
                            "codec"  => StreamRule::Codec  { op: SetOp::In, values: vec![] },
                            _        => StreamRule::Resolution { op: SetOp::In, values: vec![] },
                        };
                    }
                },
                option { value: "resolution", selected: field_val == "resolution", "Resolution" }
                option { value: "quality",     selected: field_val == "quality",     "Quality" }
                option { value: "codec",      selected: field_val == "codec",      "Codec" }
            }
            // Operator selector
            select {
                class: "select-input",
                style: "flex:1",
                onchange: move |e| {
                    let new_op = if e.value() == "not_in" { SetOp::NotIn } else { SetOp::In };
                    if let Some(r) = rules.write().get_mut(idx) {
                        *r = match r.clone() {
                            StreamRule::Resolution { values, .. } => StreamRule::Resolution { op: new_op, values },
                            StreamRule::Quality { values, .. }     => StreamRule::Quality { op: new_op, values },
                            StreamRule::Codec { values, .. }      => StreamRule::Codec  { op: new_op, values },
                        };
                    }
                },
                option { value: "in",     selected: !op_not_in, "In" }
                option { value: "not_in", selected:  op_not_in, "Not in" }
            }
            // Value checkboxes
            div { style: "flex:2;display:flex;flex-wrap:wrap;gap:6px;padding-top:2px",
                if field_val == "resolution" {
                    for res in StreamResolution::all() {
                        {
                            let res = res.clone();
                            let checked = match &rule { StreamRule::Resolution { values, .. } => values.contains(&res), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Resolution { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&res) { values.push(res.clone()); } }
                                                else { values.retain(|r| r != &res); }
                                            }
                                        },
                                    }
                                    "{res.label()}"
                                }
                            }
                        }
                    }
                } else if field_val == "quality" {
                    for src in StreamQuality::all() {
                        {
                            let src = src.clone();
                            let checked = match &rule { StreamRule::Quality { values, .. } => values.contains(&src), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Quality { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&src) { values.push(src.clone()); } }
                                                else { values.retain(|s| s != &src); }
                                            }
                                        },
                                    }
                                    "{src.label()}"
                                }
                            }
                        }
                    }
                } else {
                    for codec in StreamCodec::all() {
                        {
                            let codec = codec.clone();
                            let checked = match &rule { StreamRule::Codec { values, .. } => values.contains(&codec), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Codec { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&codec) { values.push(codec.clone()); } }
                                                else { values.retain(|c| c != &codec); }
                                            }
                                        },
                                    }
                                    "{codec.label()}"
                                }
                            }
                        }
                    }
                }
            }
            // Remove button
            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "padding:4px 8px;color:var(--text-muted)",
                onclick: move |_| {
                    let mut r = rules.write();
                    if idx < r.len() { r.remove(idx); }
                },
                "✕"
            }
        }
    }
}

#[component]
fn StreamFilterEditor(
    match_mode: Signal<FilterMatchMode>,
    rules: Signal<Vec<StreamRule>>,
) -> Element {
    let rule_count = rules.read().len();
    rsx! {
        div {
            style: "background:var(--bg);border:1px solid var(--border);border-left:3px solid var(--warning);border-radius:8px;padding:12px 14px",
            div { style: "display:flex;align-items:center;justify-content:space-between;margin-bottom:8px",
                label { class: "field-label", style: "margin:0", "Stream Filters" }
                if rule_count > 1 {
                    div { style: "display:flex;align-items:center;gap:6px",
                        span { style: "font-size:.78rem;color:var(--text-muted)", "Match:" }
                        button {
                            style: "font-size:.72rem;height:26px;padding:0 10px",
                            class: if *match_mode.read() == FilterMatchMode::All { "btn btn-primary" } else { "btn btn-ghost" },
                            onclick: move |_| match_mode.set(FilterMatchMode::All),
                            "All (AND)"
                        }
                        button {
                            style: "font-size:.72rem;height:26px;padding:0 10px",
                            class: if *match_mode.read() == FilterMatchMode::Any { "btn btn-primary" } else { "btn btn-ghost" },
                            onclick: move |_| match_mode.set(FilterMatchMode::Any),
                            "Any (OR)"
                        }
                    }
                }
            }
            for (idx, rule) in rules.read().iter().enumerate() {
                StreamRuleRow { key: "{idx}", idx, rule: rule.clone(), rules }
            }
            button {
                class: "btn btn-ghost",
                style: "margin-top:6px;font-size:.75rem;height:28px",
                onclick: move |_| {
                    rules.write().push(StreamRule::Resolution { op: SetOp::In, values: vec![] });
                },
                "+ Add Filter"
            }
        }
    }
}

#[component]
fn StreamGroupsCard(app_state: AppState) -> Element {
    let mut groups: Signal<Vec<StreamGroupDto>> = use_signal(Vec::new);
    let mut show_ungrouped = use_signal(|| true);
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0_u32);

    // Create modal state
    let mut show_create = use_signal(|| false);
    let mut create_name = use_signal(String::new);
    let mut create_match: Signal<FilterMatchMode> = use_signal(|| FilterMatchMode::All);
    let mut create_rules: Signal<Vec<StreamRule>> = use_signal(Vec::new);
    let mut create_priority = use_signal(|| 0_i64);
    let mut creating = use_signal(|| false);

    // Edit modal state
    let mut id_to_edit: Signal<Option<Uuid>> = use_signal(|| None);
    let mut edit_name = use_signal(String::new);
    let mut edit_match: Signal<FilterMatchMode> = use_signal(|| FilterMatchMode::All);
    let mut edit_rules: Signal<Vec<StreamRule>> = use_signal(Vec::new);
    let mut edit_priority = use_signal(|| 0_i64);
    let mut edit_enabled = use_signal(|| true);
    let mut edit_hidden = use_signal(|| false);
    let mut editing = use_signal(|| false);

    // Delete modal state
    let mut id_to_delete: Signal<Option<Uuid>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let mut saving_setting = use_signal(|| false);

    // Preview state
    let mut preview_imdb = use_signal(|| "tt0133093".to_string());
    let mut preview_data: Signal<Option<StreamGroupPreviewDto>> = use_signal(|| None);
    let mut preview_loading = use_signal(|| false);
    let mut preview_error: Signal<Option<String>> = use_signal(|| None);

    let app_state_preview = app_state.clone();
    use_effect(move || {
        let imdb = preview_imdb.read().clone();
        let _r = *refresh.read();
        if imdb.is_empty() {
            return;
        }
        preview_loading.set(true);
        preview_data.set(None);
        preview_error.set(None);
        let client = app_state_preview.client.clone();
        spawn(async move {
            match client
                .execute(GetStreamGroupPreview { imdb_id: imdb })
                .await
            {
                Ok(data) => {
                    preview_data.set(Some(data));
                }
                Err(e) => {
                    preview_error.set(Some(format!("{e}")));
                }
            }
            preview_loading.set(false);
        });
    });

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            let groups_res = client.execute(ListStreamGroups).await;
            let cfg_res = client.execute(GetSystemConfiguration).await;
            match (groups_res, cfg_res) {
                (Ok(g), Ok(cfg)) => {
                    show_ungrouped
                        .set(cfg.stream_groups_show_ungrouped.unwrap_or(true));
                    base_cfg.set(Some(cfg));
                    groups.set(g);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load: {e}")));
                }
            }
            loading.set(false);
        });
    });

    rsx! {
        // Settings card
        div { class: "card", style: "margin-bottom:16px",
            div { class: "card-header",
                span { class: "card-title", "Settings" }
            }
            div { class: "card-body",
                div {
                    class: "flex items-center justify-between",
                    style: "padding:8px 0",
                    div {
                        div { style: "font-size:.85rem;font-weight:500", "Show ungrouped streams" }
                        div { style: "font-size:.75rem;color:var(--text-muted)",
                            "Show streams that don't match any group as individual entries."
                        }
                    }
                    div { class: "flex items-center gap-2",
                        if *saving_setting.read() {
                            span { style: "font-size:.72rem;color:var(--text-muted)", "Saving…" }
                        }
                        input {
                            r#type: "checkbox",
                            checked: *show_ungrouped.read(),
                            disabled: *saving_setting.read(),
                            onchange: {
                                let client = app_state.client.clone();
                                move |e: Event<FormData>| {
                                    let checked = e.checked();
                                    show_ungrouped.set(checked);
                                    let Some(cfg) = base_cfg.peek().clone() else { return };
                                    let updated = ServerConfiguration {
                                        stream_groups_show_ungrouped: Some(checked),
                                        ..cfg
                                    };
                                    saving_setting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        let _ = c.execute(UpdateSystemConfiguration { config: updated }).await;
                                        saving_setting.set(false);
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Groups card
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Stream Groups" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        create_name.set(String::new());
                        create_match.set(FilterMatchMode::All);
                        create_rules.set(vec![]);
                        create_priority.set(0);
                        show_create.set(true);
                    },
                    "+ New Group"
                }
            }
            div { class: "card-body tight",

                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if groups.read().is_empty() {
                    div { class: "empty-state",
                        "No stream groups — create one to consolidate similar streams."
                    }
                } else {
                    div { class: "row-list",
                        for group in groups.read().clone() {
                            {
                                let gid = group.id;
                                let gid_del = group.id;
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                        key: "{group.id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem", "{group.name}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:3px;display:flex;flex-wrap:wrap;gap:4px",
                                                for rule in group.filter.rules.iter() {
                                                    {
                                                        let (label, is_excl, color_style) = match rule {
                                                            StreamRule::Resolution { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:var(--accent-subtle,rgba(99,102,241,.12));color:var(--accent,#6366f1);padding:1px 6px;border-radius:4px")
                                                            }
                                                            StreamRule::Quality { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:rgba(0,0,0,0.06);padding:1px 6px;border-radius:4px")
                                                            }
                                                            StreamRule::Codec { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:rgba(16,185,129,.12);color:rgb(5,150,105);padding:1px 6px;border-radius:4px")
                                                            }
                                                        };
                                                        let prefix = if is_excl { "NOT " } else { "" };
                                                        rsx! { span { style: "{color_style}", "{prefix}{label}" } }
                                                    }
                                                }
                                                if group.filter.rules.len() > 1 {
                                                    span { style: "color:var(--text-muted);font-style:italic",
                                                        {if group.filter.match_mode == FilterMatchMode::All { "AND" } else { "OR" }}
                                                    }
                                                }
                                                span { style: "color:var(--text-muted)", "priority {group.priority}" }
                                                if !group.enabled {
                                                    span { style: "color:var(--error)", "disabled" }
                                                }
                                            }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    edit_name.set(group.name.clone());
                                                    edit_match.set(group.filter.match_mode.clone());
                                                    edit_rules.set(group.filter.rules.clone());
                                                    edit_priority.set(group.priority);
                                                    edit_enabled.set(group.enabled);
                                                    edit_hidden.set(group.hidden);
                                                    id_to_edit.set(Some(gid));
                                                },
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| id_to_delete.set(Some(gid_del)),
                                                "Delete"
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

        // Preview card — only shown when at least one group is configured
        if !groups.read().is_empty() {
            div { class: "card", style: "margin-top:16px",
                div { class: "card-header",
                    span { class: "card-title", "Example output" }
                }
                div { class: "card-body",
                    div { class: "form-group", style: "margin-bottom:12px",
                        label { style: "font-size:.75rem;font-weight:500;display:block;margin-bottom:4px",
                            "IMDB ID"
                        }
                        input {
                            r#type: "text",
                            class: "input",
                            style: "width:180px",
                            value: "{preview_imdb}",
                            oninput: move |e| preview_imdb.set(e.value()),
                        }
                    }
                    if *preview_loading.read() {
                        div { style: "font-size:.8rem;color:var(--text-muted)", "Loading…" }
                    } else if let Some(ref err) = *preview_error.read() {
                        div { style: "font-size:.8rem;color:var(--error)", "{err}" }
                    } else if let Some(ref data) = *preview_data.read() {
                        div { style: "font-family:monospace;font-size:.78rem;line-height:1.6",
                            if data.groups.is_empty() && data.ungrouped.is_empty() {
                                div { style: "color:var(--text-muted)", "No streams returned for this IMDB ID." }
                            }
                            for group in &data.groups {
                                div { style: "margin-bottom:6px",
                                    div { style: "font-weight:600;display:flex;align-items:center;gap:6px",
                                        "▼ {group.name}"
                                        if group.hidden {
                                            span {
                                                style: "font-size:.68rem;padding:1px 5px;border-radius:3px;background:var(--bg-subtle,#333);color:var(--text-muted);font-family:sans-serif",
                                                "hidden"
                                            }
                                        }
                                    }
                                    for stream in &group.streams {
                                        div { style: "padding-left:16px;color:var(--text-muted)",
                                            "└ {stream}"
                                        }
                                    }
                                }
                            }
                            if !data.ungrouped.is_empty() {
                                div { style: "margin-top:4px",
                                    div { style: "font-weight:600", "─ Ungrouped" }
                                    for stream in &data.ungrouped {
                                        div { style: "padding-left:16px;color:var(--text-muted)",
                                            "└ {stream}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Create modal
        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "New Stream Group" }
                    }
                    div { class: "modal-body",
                        div { class: "form-group",
                            label { class: "form-label", "Name" }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "Auto-generated from filter",
                                value: "{create_name}",
                                oninput: move |e| create_name.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Filter rules" }
                            StreamFilterEditor { match_mode: create_match, rules: create_rules }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Priority (lower = shown first)" }
                            input {
                                class: "form-input",
                                r#type: "number",
                                value: "{create_priority}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        create_priority.set(n);
                                    }
                                },
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *creating.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let name = create_name.read().trim().to_string();
                                    creating.set(true);
                                    let c = client.clone();
                                    let filter = StreamFilter {
                                        match_mode: create_match.peek().clone(),
                                        rules: create_rules.peek().clone(),
                                    };
                                    let prio = *create_priority.peek();
                                    spawn(async move {
                                        match c.execute(CreateStreamGroup {
                                            payload: CreateStreamGroupRequest {
                                                name,
                                                filter,
                                                priority: prio,
                                            },
                                        }).await {
                                            Ok(_) => {
                                                show_create.set(false);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to create: {e}")));
                                                show_create.set(false);
                                            }
                                        }
                                        creating.set(false);
                                    });
                                }
                            },
                            if *creating.read() { "Creating…" } else { "Create" }
                        }
                    }
                }
            }
        }

        // Edit modal
        if id_to_edit.read().is_some() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Edit Stream Group" }
                    }
                    div { class: "modal-body",
                        div { class: "form-group",
                            label { class: "form-label", "Name" }
                            input {
                                class: "form-input",
                                r#type: "text",
                                value: "{edit_name}",
                                oninput: move |e| edit_name.set(e.value()),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Filter rules" }
                            StreamFilterEditor { match_mode: edit_match, rules: edit_rules }
                        }
                        div { class: "form-group",
                            label { class: "form-label", "Priority (lower = shown first)" }
                            input {
                                class: "form-input",
                                r#type: "number",
                                value: "{edit_priority}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        edit_priority.set(n);
                                    }
                                },
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *edit_enabled.read(),
                                    onchange: move |e| edit_enabled.set(e.checked()),
                                }
                                "Enabled"
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *edit_hidden.read(),
                                    onchange: move |e| edit_hidden.set(e.checked()),
                                }
                                "Hide group"
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| id_to_edit.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *editing.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let Some(id) = *id_to_edit.peek() else { return };
                                    let name = edit_name.read().trim().to_string();
                                    editing.set(true);
                                    let c = client.clone();
                                    let filter = StreamFilter {
                                        match_mode: edit_match.peek().clone(),
                                        rules: edit_rules.peek().clone(),
                                    };
                                    let prio = *edit_priority.peek();
                                    let enabled = *edit_enabled.peek();
                                    let hidden = *edit_hidden.peek();
                                    spawn(async move {
                                        match c.execute(UpdateStreamGroup {
                                            id,
                                            payload: UpdateStreamGroupRequest {
                                                name,
                                                filter,
                                                priority: prio,
                                                enabled,
                                                hidden,
                                            },
                                        }).await {
                                            Ok(_) => {
                                                id_to_edit.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to update: {e}")));
                                                id_to_edit.set(None);
                                            }
                                        }
                                        editing.set(false);
                                    });
                                }
                            },
                            if *editing.read() { "Saving…" } else { "Save" }
                        }
                    }
                }
            }
        }

        // Delete confirm modal
        if id_to_delete.read().is_some() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Delete Stream Group" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.85rem",
                            "Are you sure you want to delete this stream group? This cannot be undone."
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            disabled: *deleting.read(),
                            onclick: move |_| id_to_delete.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            style: "background:var(--error);border-color:var(--error)",
                            disabled: *deleting.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let Some(id) = *id_to_delete.peek() else { return };
                                    deleting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(DeleteStreamGroup { id }).await {
                                            Ok(_) => {
                                                id_to_delete.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to delete: {e}")));
                                                id_to_delete.set(None);
                                            }
                                        }
                                        deleting.set(false);
                                    });
                                }
                            },
                            if *deleting.read() { "Deleting…" } else { "Delete" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AddonsPage(app_state: AppState) -> Element {
    let mut addons: Signal<Vec<AddonDto>> = use_signal(Vec::new);
    let mut kinds: Signal<Vec<AddonMetadata>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0_u32);

    // Add-addon modal state
    let mut show_create = use_signal(|| false);
    let mut create_step: Signal<u8> = use_signal(|| 0); // 0 = pick kind, 1 = configure
    let mut selected_kind: Signal<Option<String>> = use_signal(|| None);
    let mut name_input = use_signal(String::new);
    // Form values keyed by option id; stored as serde_json::Value to round-trip cleanly.
    let mut form_values: Signal<std::collections::HashMap<String, serde_json::Value>> =
        use_signal(std::collections::HashMap::new);
    let mut creating = use_signal(|| false);

    // Edit-addon modal state
    let mut id_to_edit: Signal<Option<Uuid>> = use_signal(|| None);
    let mut edit_name_input = use_signal(String::new);
    let mut edit_form_values: Signal<
        std::collections::HashMap<String, serde_json::Value>,
    > = use_signal(std::collections::HashMap::new);
    let mut editing = use_signal(|| false);
    // Resources checked state for edit form (set of enabled ResourceType display strings)
    let mut edit_resources: Signal<std::collections::HashSet<String>> =
        use_signal(std::collections::HashSet::new);
    // Types checked state for edit form (set of enabled MediaKind display strings)
    let mut edit_types: Signal<std::collections::HashSet<String>> =
        use_signal(std::collections::HashSet::new);
    // Catalogs loaded for the addon being edited
    let mut edit_catalogs: Signal<Vec<AddonCatalogDto>> = use_signal(Vec::new);
    let mut edit_catalogs_loading = use_signal(|| false);
    // Per-catalog overrides: catalog_id -> (enabled, max_items_str)
    let mut edit_catalog_settings: Signal<
        std::collections::HashMap<String, (bool, String)>,
    > = use_signal(std::collections::HashMap::new);

    // Confirm-delete state
    let mut id_to_delete: Signal<Option<Uuid>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            let kinds_res = client.execute(ListAddonKinds).await;
            let addons_res = client.execute(ListAddons).await;
            match (kinds_res, addons_res) {
                (Ok(k), Ok(a)) => {
                    kinds.set(k);
                    addons.set(a);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load addons: {e}")));
                }
            }
            loading.set(false);
        });
    });

    let selected_kind_meta = {
        let sel = selected_kind.read().clone();
        sel.and_then(|id| kinds.read().iter().find(|k| k.id == id).cloned())
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Addons" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        name_input.set(String::new());
                        form_values.set(std::collections::HashMap::new());
                        selected_kind.set(None);
                        create_step.set(0);
                        show_create.set(true);
                    },
                    "+ New Addon"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if addons.read().is_empty() {
                    div { class: "empty-state", "No addons configured — add one to get started." }
                } else {
                    div { class: "addon-list",
                        for (addon_idx, addon) in addons.read().clone().into_iter().enumerate() {
                            {
                                let id = addon.id;
                                let addon_count = addons.read().len();
                                rsx! {
                                    div { class: "addon-card", key: "{id}",
                                        div { class: "addon-card-header",
                                            span { class: "addon-card-name", "{addon.name}" }
                                            span { class: "addon-card-kind", "{addon.kind}" }
                                        }
                                        div { class: "addon-kind-card-badges",
                                            for res in addon.resources.iter() {
                                                span { class: "addon-kind-badge", "{res:?}" }
                                            }
                                            {
                                                let display_types = if addon.types.is_empty() { &addon.supported_types } else { &addon.types };
                                                rsx! {
                                                    for t in display_types.iter() {
                                                        span { class: "addon-kind-type", "{t}" }
                                                    }
                                                }
                                            }
                                        }
                                        div { class: "addon-card-actions",
                                            // Up/down reorder buttons
                                            div { class: "addon-card-sort",
                                                button {
                                                    class: "btn btn-ghost addon-sort-btn",
                                                    disabled: addon_idx == 0,
                                                    title: "Move up (higher priority)",
                                                    onclick: {
                                                        let client = app_state.client.clone();
                                                        move |_| {
                                                            let current = addons.read().clone();
                                                            if addon_idx == 0 { return; }
                                                            let mut new_order = current.clone();
                                                            new_order.swap(addon_idx, addon_idx - 1);
                                                            let updates: Vec<(Uuid, i64)> = new_order.iter().enumerate()
                                                                .filter_map(|(i, a)| {
                                                                    let new_prio = i as i64 * 10;
                                                                    if a.priority != new_prio { Some((a.id, new_prio)) } else { None }
                                                                })
                                                                .collect();
                                                            let c = client.clone();
                                                            spawn(async move {
                                                                for (uid, prio) in updates {
                                                                    let _ = c.execute(UpdateAddon { id: uid, payload: UpdateAddonRequest { priority: Some(prio), ..Default::default() } }).await;
                                                                }
                                                                let v = *refresh.peek() + 1;
                                                                refresh.set(v);
                                                            });
                                                        }
                                                    },
                                                    "↑"
                                                }
                                                button {
                                                    class: "btn btn-ghost addon-sort-btn",
                                                    disabled: addon_idx + 1 >= addon_count,
                                                    title: "Move down (lower priority)",
                                                    onclick: {
                                                        let client = app_state.client.clone();
                                                        move |_| {
                                                            let current = addons.read().clone();
                                                            if addon_idx + 1 >= current.len() { return; }
                                                            let mut new_order = current.clone();
                                                            new_order.swap(addon_idx, addon_idx + 1);
                                                            let updates: Vec<(Uuid, i64)> = new_order.iter().enumerate()
                                                                .filter_map(|(i, a)| {
                                                                    let new_prio = i as i64 * 10;
                                                                    if a.priority != new_prio { Some((a.id, new_prio)) } else { None }
                                                                })
                                                                .collect();
                                                            let c = client.clone();
                                                            spawn(async move {
                                                                for (uid, prio) in updates {
                                                                    let _ = c.execute(UpdateAddon { id: uid, payload: UpdateAddonRequest { priority: Some(prio), ..Default::default() } }).await;
                                                                }
                                                                let v = *refresh.peek() + 1;
                                                                refresh.set(v);
                                                            });
                                                        }
                                                    },
                                                    "↓"
                                                }
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:28px;font-size:.68rem;padding:0 10px",
                                                onclick: {
                                                    let client = app_state.client.clone();
                                                    move |_| {
                                                        if let Some(a) = addons.read().iter().find(|a| a.id == id).cloned() {
                                                            edit_name_input.set(a.name.clone());
                                                            let config_map = a.config.as_object()
                                                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                                                .unwrap_or_default();
                                                            edit_form_values.set(config_map);
                                                            let res_set: std::collections::HashSet<String> = a.resources
                                                                .iter()
                                                                .map(|r| format!("{r}"))
                                                                .collect();
                                                            edit_resources.set(res_set);
                                                            // Empty types = all enabled — pre-check every supported type.
                                                            let type_set: std::collections::HashSet<String> = if a.types.is_empty() {
                                                                a.supported_types.iter().map(|t| format!("{t}")).collect()
                                                            } else {
                                                                a.types.iter().map(|t| format!("{t}")).collect()
                                                            };
                                                            edit_types.set(type_set);
                                                            let has_catalog = a.resources.contains(&ResourceType::Catalog);
                                                            edit_catalogs.set(Vec::new());
                                                            edit_catalog_settings.set(std::collections::HashMap::new());
                                                            id_to_edit.set(Some(id));
                                                            if has_catalog {
                                                                edit_catalogs_loading.set(true);
                                                                let c = client.clone();
                                                                spawn(async move {
                                                                    match c.execute(GetAddonCatalogs { id }).await {
                                                                        Ok(cats) => {
                                                                            let settings: std::collections::HashMap<String, (bool, String)> = cats
                                                                                .iter()
                                                                                .map(|cat| (
                                                                                    cat.catalog_id.clone(),
                                                                                    (cat.enabled, cat.max_items.map(|n| n.to_string()).unwrap_or_default()),
                                                                                ))
                                                                                .collect();
                                                                            edit_catalog_settings.set(settings);
                                                                            edit_catalogs.set(cats);
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(format!("Failed to load catalogs: {e}")));
                                                                        }
                                                                    }
                                                                    edit_catalogs_loading.set(false);
                                                                });
                                                            }
                                                        }
                                                    }
                                                },
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:28px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| id_to_delete.set(Some(id)),
                                                "Delete"
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

        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal modal--wide",
                    div { class: "modal-header",
                        span { class: "modal-title",
                            if *create_step.read() == 0 { "Choose Type" } else { "Configure Addon" }
                        }
                    }
                    div { class: "modal-body",
                        if *create_step.read() == 0 {
                            // ── Step 1: kind picker ──
                            div { class: "addon-kind-list",
                                for k in kinds.read().clone() {
                                    {
                                        let k_id = k.id.clone();
                                        let k_name = k.display_name.clone();
                                        let is_selected = selected_kind.read().as_deref() == Some(&k.id);
                                        rsx! {
                                            div {
                                                class: if is_selected { "addon-kind-card addon-kind-card--selected" } else { "addon-kind-card" },
                                                onclick: move |_| {
                                                    selected_kind.set(Some(k_id.clone()));
                                                    form_values.set(std::collections::HashMap::new());
                                                },
                                                div { class: "addon-kind-card-name", "{k.display_name}" }
                                                div { class: "addon-kind-card-desc", "{k.description}" }
                                                div { class: "addon-kind-card-badges",
                                                    for res in k.supported_resources.iter() {
                                                        span { class: "addon-kind-badge", "{res:?}" }
                                                    }
                                                    for t in k.supported_types.iter() {
                                                        span { class: "addon-kind-type", "{t}" }
                                                    }
                                                }
                                                if is_selected {
                                                    button {
                                                        class: "btn btn-primary addon-kind-card-configure",
                                                        onclick: move |e| {
                                                            e.stop_propagation();
                                                            name_input.set(k_name.clone());
                                                            create_step.set(1);
                                                        },
                                                        "Configure →"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // ── Step 2: name + options ──
                            if let Some(meta) = &selected_kind_meta {
                                div { class: "field-hint", style: "margin-bottom:4px", "{meta.description}" }
                            }
                            div { class: "form-group",
                                label { class: "form-label", "Name" }
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "Display name",
                                    value: "{name_input}",
                                    oninput: move |e| name_input.set(e.value()),
                                }
                            }
                            if let Some(meta) = &selected_kind_meta {
                                for opt in meta.options.iter().cloned() {
                                    AddonOptionField {
                                        option: opt,
                                        values: form_values,
                                    }
                                }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| {
                                if *create_step.read() == 1 {
                                    create_step.set(0);
                                } else {
                                    show_create.set(false);
                                }
                            },
                            if *create_step.read() == 1 { "← Back" } else { "Cancel" }
                        }
                        if *create_step.read() == 1 {
                            button {
                                class: "btn btn-primary",
                                disabled: *creating.read() || name_input.read().trim().is_empty() || selected_kind.read().is_none(),
                                onclick: {
                                    let client = app_state.client.clone();
                                    move |_| {
                                        let name = name_input.read().trim().to_string();
                                        let Some(kind) = selected_kind.read().clone() else { return; };
                                        if name.is_empty() { return; }
                                        let config: serde_json::Value = serde_json::Value::Object(
                                            form_values.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                        );
                                        creating.set(true);
                                        let c = client.clone();
                                        spawn(async move {
                                            let payload = CreateAddonRequest {
                                                preset: AddonPresetRef { kind, config },
                                                name,
                                                resources: Vec::new(),
                                                types: Vec::new(),
                                                priority: 0,
                                            };
                                            match c.execute(CreateAddon { payload }).await {
                                                Ok(_) => {
                                                    show_create.set(false);
                                                    let v = *refresh.peek() + 1;
                                                    refresh.set(v);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to create addon: {e}")));
                                                }
                                            }
                                            creating.set(false);
                                        });
                                    }
                                },
                                if *creating.read() { "Creating…" } else { "Create" }
                            }
                        }
                    }
                }
            }
        }

        if let Some(edit_id) = *id_to_edit.read() {
            {
                let edit_kind = addons.read().iter().find(|a| a.id == edit_id).map(|a| a.kind.clone());
                let edit_kind_meta = edit_kind.as_ref().and_then(|k| kinds.read().iter().find(|m| m.id == *k).cloned());
                // Use supported_resources from the addon row (manifest-derived for Stremio,
                // kind-static for others) as the checkbox option list.
                let resource_options: Vec<ResourceType> = addons
                    .read()
                    .iter()
                    .find(|a| a.id == edit_id)
                    .map(|a| a.supported_resources.clone())
                    .unwrap_or_default();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "Edit Addon" }
                            }
                            div { class: "modal-body",
                                div { class: "form-group",
                                    label { class: "form-label", "Name" }
                                    input {
                                        class: "form-input",
                                        r#type: "text",
                                        placeholder: "Display name",
                                        value: "{edit_name_input}",
                                        oninput: move |e| edit_name_input.set(e.value()),
                                    }
                                }
                                if let Some(meta) = &edit_kind_meta {
                                    for opt in meta.options.iter().cloned() {
                                        AddonOptionField {
                                            option: opt,
                                            values: edit_form_values,
                                        }
                                    }
                                }
                                // Resources section — options come from the addon row.
                                if !resource_options.is_empty() {
                                    div { class: "form-group",
                                        label { class: "form-label", "Resources" }
                                        div { class: "check-row-group",
                                            for res in resource_options.iter().cloned() {
                                                {
                                                    let res_str = format!("{res}");
                                                    let res_str_check = res_str.clone();
                                                    let checked = edit_resources.read().contains(&res_str);
                                                    rsx! {
                                                        label { class: "check-row",
                                                            input {
                                                                r#type: "checkbox",
                                                                checked,
                                                                onchange: move |e| {
                                                                    let mut set = edit_resources.write();
                                                                    if e.checked() {
                                                                        set.insert(res_str_check.clone());
                                                                    } else {
                                                                        set.remove(&res_str_check);
                                                                    }
                                                                },
                                                            }
                                                            "{res_str}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                // Types section
                                {
                                    let type_options: Vec<remux_sdks::remux::MediaKind> = addons
                                        .read()
                                        .iter()
                                        .find(|a| a.id == edit_id)
                                        .map(|a| a.supported_types.clone())
                                        .unwrap_or_default();
                                    if !type_options.is_empty() {
                                        rsx! {
                                            div { class: "form-group",
                                                label { class: "form-label", "Content Types" }
                                                div { class: "check-row-group",
                                                    for t in type_options.into_iter() {
                                                        {
                                                            let t_str = format!("{t}");
                                                            let t_str_check = t_str.clone();
                                                            let checked = edit_types.read().contains(&t_str);
                                                            rsx! {
                                                                label { class: "check-row",
                                                                    input {
                                                                        r#type: "checkbox",
                                                                        checked,
                                                                        onchange: move |e| {
                                                                            let mut set = edit_types.write();
                                                                            if e.checked() {
                                                                                set.insert(t_str_check.clone());
                                                                            } else {
                                                                                set.remove(&t_str_check);
                                                                            }
                                                                        },
                                                                    }
                                                                    "{t_str}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        rsx! {}
                                    }
                                }
                                // Catalogs section (only shown when catalog resource is active)
                                if edit_resources.read().contains("catalog") {
                                    div { class: "form-group",
                                        label { class: "form-label", "Catalogs" }
                                        if *edit_catalogs_loading.read() {
                                            span { class: "field-hint", "Loading catalogs…" }
                                        } else if edit_catalogs.read().is_empty() {
                                            span { class: "field-hint", "No catalogs found." }
                                        } else {
                                            div { class: "catalog-table-wrap",
                                                table { class: "catalog-table",
                                                    thead {
                                                        tr {
                                                            th { "Catalog" }
                                                            th { "Enabled" }
                                                            th { "Max items" }
                                                        }
                                                    }
                                                    tbody {
                                                        for cat in edit_catalogs.read().clone() {
                                                            {
                                                                let cid = cat.catalog_id.clone();
                                                                let cid_toggle = cid.clone();
                                                                let cid_max = cid.clone();
                                                                let (enabled, max_str) = edit_catalog_settings.read()
                                                                    .get(&cid)
                                                                    .cloned()
                                                                    .unwrap_or((false, String::new()));
                                                                rsx! {
                                                                    tr {
                                                                        td { class: "catalog-name", "{cat.name}" }
                                                                        td {
                                                                            input {
                                                                                r#type: "checkbox",
                                                                                checked: enabled,
                                                                                onchange: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_toggle.clone()).or_default();
                                                                                    entry.0 = e.checked();
                                                                                },
                                                                            }
                                                                        }
                                                                        td {
                                                                            input {
                                                                                r#type: "number",
                                                                                placeholder: "Max items",
                                                                                value: "{max_str}",
                                                                                min: "1",
                                                                                oninput: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_max.clone()).or_default();
                                                                                    entry.1 = e.value();
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
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| id_to_edit.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: *editing.read() || edit_name_input.read().trim().is_empty(),
                                    onclick: {
                                        let client = app_state.client.clone();
                                        move |_| {
                                            let name = edit_name_input.read().trim().to_string();
                                            if name.is_empty() { return; }
                                            let config: serde_json::Value = serde_json::Value::Object(
                                                edit_form_values.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                            );
                                            // Build resources list from checkboxes.
                                            let resources: Vec<ResourceType> = edit_resources
                                                .read()
                                                .iter()
                                                .filter_map(|s| s.parse::<ResourceType>().ok())
                                                .collect();
                                            let types: Vec<remux_sdks::remux::MediaKind> = edit_types
                                                .read()
                                                .iter()
                                                .filter_map(|s| s.parse::<remux_sdks::remux::MediaKind>().ok())
                                                .collect();
                                            // Build catalog update payload.
                                            let catalog_updates: Vec<UpdateAddonCatalogRequest> = edit_catalog_settings
                                                .read()
                                                .iter()
                                                .map(|(catalog_id, (enabled, max_str))| UpdateAddonCatalogRequest {
                                                    catalog_id: catalog_id.clone(),
                                                    enabled: *enabled,
                                                    max_items: max_str.trim().parse::<i64>().ok().filter(|&n| n > 0),
                                                })
                                                .collect();
                                            editing.set(true);
                                            let c = client.clone();
                                            spawn(async move {
                                                let payload = UpdateAddonRequest {
                                                    name: Some(name),
                                                    config: Some(config),
                                                    resources: Some(resources),
                                                    types: Some(types),
                                                    enabled: None,
                                                    priority: None,
                                                };
                                                let addon_res = c.execute(UpdateAddon { id: edit_id, payload }).await;
                                                let cat_res = if !catalog_updates.is_empty() {
                                                    c.execute(UpdateAddonCatalogs { id: edit_id, payload: catalog_updates }).await.err()
                                                } else {
                                                    None
                                                };
                                                match (addon_res, cat_res) {
                                                    (Ok(_), None) => {
                                                        id_to_edit.set(None);
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    }
                                                    (Ok(_), Some(e)) => {
                                                        error.set(Some(format!("Addon saved but catalog update failed: {e}")));
                                                        id_to_edit.set(None);
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    }
                                                    (Err(e), _) => {
                                                        error.set(Some(format!("Failed to update addon: {e}")));
                                                    }
                                                }
                                                editing.set(false);
                                            });
                                        }
                                    },
                                    if *editing.read() { "Saving…" } else { "Save" }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(del_id) = *id_to_delete.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Delete Addon" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.85rem", "Are you sure you want to delete this addon? Catalogs from this addon will be removed on the next import." }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| id_to_delete.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *deleting.read(),
                            style: "background:var(--error);border-color:var(--error)",
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    deleting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(DeleteAddon { id: del_id }).await {
                                            Ok(_) => {
                                                id_to_delete.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to delete addon: {e}")));
                                            }
                                        }
                                        deleting.set(false);
                                    });
                                }
                            },
                            if *deleting.read() { "Deleting…" } else { "Delete" }
                        }
                    }
                }
            }
        }
    }
}

/// Generic form-field renderer driven by an [`AddonOption`] descriptor.
/// Stores the current value back into a shared `values` map keyed by option id.
#[component]
fn AddonOptionField(
    option: AddonOption,
    values: Signal<std::collections::HashMap<String, serde_json::Value>>,
) -> Element {
    let id = option.id.clone();
    let label = option.name.clone();
    let desc = option.description.clone();
    let id_change = id.clone();
    let id_check = id.clone();
    let id_num = id.clone();
    let id_pwd = id.clone();
    let id_text = id.clone();
    let id_select = id.clone();

    let current_str = values
        .read()
        .get(&id)
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| Some(v.to_string()))
        })
        .unwrap_or_default();
    let current_bool = values
        .read()
        .get(&id)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    rsx! {
        div { class: "form-group",
            label { class: "form-label", "{label}" }
            match &option.kind {
                AddonOptionType::Url | AddonOptionType::String => rsx! {
                    input {
                        class: "form-input",
                        r#type: "text",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_change.clone(), serde_json::Value::String(e.value()));
                        },
                    }
                },
                AddonOptionType::Password => rsx! {
                    input {
                        class: "form-input",
                        r#type: "password",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_pwd.clone(), serde_json::Value::String(e.value()));
                        },
                    }
                },
                AddonOptionType::Textarea => rsx! {
                    textarea {
                        class: "form-input",
                        rows: 4,
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_text.clone(), serde_json::Value::String(e.value()));
                        },
                        "{current_str}"
                    }
                },
                AddonOptionType::Number { .. } => rsx! {
                    input {
                        class: "form-input",
                        r#type: "number",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            if let Ok(n) = e.value().parse::<i64>() {
                                map.insert(id_num.clone(), serde_json::json!(n));
                            }
                        },
                    }
                },
                AddonOptionType::Boolean => rsx! {
                    label { class: "form-toggle",
                        input {
                            r#type: "checkbox",
                            checked: current_bool,
                            onchange: move |e| {
                                let mut map = values.write();
                                map.insert(id_check.clone(), serde_json::Value::Bool(e.value() == "true"));
                            },
                        }
                        span { "Enabled" }
                    }
                },
                AddonOptionType::Select { options } => rsx! {
                    select {
                        class: "form-input",
                        value: "{current_str}",
                        onchange: move |e| {
                            let mut map = values.write();
                            map.insert(id_select.clone(), serde_json::Value::String(e.value()));
                        },
                        for so in options.iter().cloned() {
                            option { value: "{so.value}", "{so.label}" }
                        }
                    }
                },
                AddonOptionType::MultiSelect { .. } | AddonOptionType::StringList => rsx! {
                    div { class: "field-hint", "(complex inputs not yet supported in dashboard)" }
                },
            }
            if let Some(d) = &desc {
                div { class: "field-hint", "{d}" }
            }
        }
    }
}
