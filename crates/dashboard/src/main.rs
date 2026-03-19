use dioxus::prelude::*;
use futures::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use shared::sdks::jellyfin::{
    AddTunerHost, AdminSetPassword, AioCatalogInfo, AuthenticateUserByName, BaseItemDto,
    BrandingOptions, ChannelEditorItem, CreateUser, CreateVirtualFolder, CreateVirtualFolderPayload,
    DeleteEpgSource, DeleteTunerHost, DeleteUser, DeleteVirtualFolder, EpgSourceInfo,
    GetAioCatalogs, GetBrandingConfiguration, GetEpgSources, GetIptvChannels,
    GetItems, GetScheduledTasks, GetSessions, GetStartupConfiguration,
    GetSystemConfiguration, GetTunerHosts, GetUsers, JellyfinAuth,
    BulkChannelRequest, BulkChannels, PatchChannel, PatchChannelRequest, PatchItem, PatchItemPayload,
    PostStartupComplete, PostStartupConfiguration, PostStartupUser, PublicSystemInfo,
    SaveEpgSource, ServerConfiguration, SessionInfoDto, SetLogLevel, StartTask, StartupConfiguration,
    StartupUser, StopTask, TaskInfo, TaskTriggerInfo, TaskTriggerInfoType, TunerHostInfo, UpdateBrandingConfiguration,
    UpdateCatalogSettings, UpdateCatalogSettingsPayload, UpdateSystemConfiguration,
    UpdateTaskTriggers, UpdateUser, UpdateUserPolicy, UserDto,
};
use shared::sdks::{ClientError, RestClient};
use uuid::Uuid;

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
        let client = shared::sdks::jellyfin::client(&server.manual_address)
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
            let needed = match shared::sdks::jellyfin::client(&origin) {
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

// ── Login ─────────────────────────────────────────────────────────────────────

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
            let reachable = match shared::sdks::jellyfin::client(&origin) {
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
            let client = match shared::sdks::jellyfin::client(&url) {
                Ok(c) => c.with_auth(JellyfinAuth::new(&device_id)),
                Err(e) => {
                    error.set(Some(format!("Bad server URL: {e}")));
                    loading.set(false);
                    return;
                }
            };

            match client
                .execute(AuthenticateUserByName { username: Some(u), pw: Some(p) })
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

// ── Dashboard ─────────────────────────────────────────────────────────────────

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
                    KvRow { label: "Version", value: info.version.clone() }
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
    let kind = t.r#type.as_deref()
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
            let hours = t.interval_ticks.unwrap_or(0) / 36_000_000_000;
            format!("Every {} hour(s)", hours)
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
    let mut new_interval_hours = use_signal(|| "24".to_string());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let task_id = task.id.clone();
    let task_name = task.name.clone();

    rsx! {
        h2 { class: "modal-title", "Triggers — {task_name}" }

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
            div { class: "field",
                label { class: "field-label", "Every (hours)" }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "1",
                    value: "{new_interval_hours.read()}",
                    oninput: move |evt| new_interval_hours.set(evt.value()),
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
                        let hours: i64 = new_interval_hours.read().parse().unwrap_or(24).max(1);
                        TaskTriggerInfo {
                            r#type: Some(kind.to_string()),
                            interval_ticks: Some(hours * 36_000_000_000),
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
                                error.set(Some(e.to_string()));
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
        let _r = *refresh.read(); // re-run effect when refresh increments
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

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", if running_only { "Running Tasks" } else { "Scheduled Tasks" } }
            }
            div { class: "card-body tight",
                if *loading.read() {
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
    #[route("/")]
    OverviewRoute,
    #[route("/imports")]
    ImportsRoute,
    #[route("/library")]
    CollectionsRoute,
    #[route("/iptv")]
    IptvRoute,
    #[route("/devices")]
    DevicesRoute,
    #[route("/tasks")]
    TasksRoute,
    #[route("/users")]
    UsersRoute,
    #[route("/settings")]
    SettingsRoute,
    #[route("/branding")]
    BrandingRoute,
    #[route("/logs")]
    LogsRoute,
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
        Route::OverviewRoute => "Overview",
        Route::ImportsRoute => "Imports",
        Route::CollectionsRoute => "Library",
        Route::IptvRoute => "IPTV",
        Route::DevicesRoute => "Devices",
        Route::TasksRoute => "Scheduled Tasks",
        Route::UsersRoute => "Users",
        Route::SettingsRoute => "Settings",
        Route::BrandingRoute => "Branding",
        Route::LogsRoute => "Logs",
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
                    span { class: "brand-label", "Remux" }
                    h1 { class: "brand-title", style: "font-size:1.1rem;margin:0", "Dashboard" }
                }

                div { class: "sidebar-nav",
                    NavItem {
                        label: "Overview",
                        active: route == Route::OverviewRoute,
                        on_click: move |_| { navigator().push(Route::OverviewRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Library",
                        active: route == Route::CollectionsRoute,
                        on_click: move |_| { navigator().push(Route::CollectionsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "IPTV",
                        active: route == Route::IptvRoute,
                        on_click: move |_| { navigator().push(Route::IptvRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Imports",
                        active: route == Route::ImportsRoute,
                        on_click: move |_| { navigator().push(Route::ImportsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Devices",
                        active: route == Route::DevicesRoute,
                        on_click: move |_| { navigator().push(Route::DevicesRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Tasks",
                        active: route == Route::TasksRoute,
                        on_click: move |_| { navigator().push(Route::TasksRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Logs",
                        active: route == Route::LogsRoute,
                        on_click: move |_| { navigator().push(Route::LogsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Users",
                        active: route == Route::UsersRoute,
                        on_click: move |_| { navigator().push(Route::UsersRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Settings",
                        active: route == Route::SettingsRoute,
                        on_click: move |_| { navigator().push(Route::SettingsRoute); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Branding",
                        active: route == Route::BrandingRoute,
                        on_click: move |_| { navigator().push(Route::BrandingRoute); sidebar_open.set(false); },
                    }
                }

                div { class: "sidebar-footer",
                    a {
                        class: "btn btn-ghost",
                        style: "width:100%;margin-bottom:8px",
                        href: "/",
                        "Home"
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

// ── Route components ────────────────────────────────────────────────
// Thin wrappers: pull AppState from context (provided by DashboardLayout)
// then pass as props to the real page components.

#[component]
fn OverviewRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! {
        ServerInfoCard { app_state: app_state.clone() }
        SessionsCard { app_state: app_state.clone() }
        TasksCard { app_state: app_state.clone(), running_only: true }
    }
}

#[component]
fn ImportsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { ImportsPage { app_state } }
}

#[component]
fn CollectionsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { CollectionsPage { app_state } }
}

#[component]
fn IptvRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { IptvPage { app_state } }
}

#[component]
fn DevicesRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SessionsCard { app_state } }
}

#[component]
fn TasksRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { TasksCard { app_state } }
}

#[component]
fn UsersRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { UsersPage { app_state } }
}

#[component]
fn SettingsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { SettingsPage { app_state } }
}

#[component]
fn BrandingRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { BrandingPage { app_state } }
}

#[component]
fn LogsRoute() -> Element {
    let app_state = use_context::<AppState>();
    rsx! { LogsPage { app_state } }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    navigator().replace(Route::OverviewRoute);
    rsx! {}
}

// ── Collections page ───────────────────────────────────────────────

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
                    include_item_types: vec![
                        "BoxSet".to_string(),
                        "CollectionFolder".to_string(),
                    ],
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
                                            shared::sdks::jellyfin::CollectionType::Movies  => "Movies",
                                            shared::sdks::jellyfin::CollectionType::Tvshows => "Shows",
                                            _ => "Unknown",
                                        },
                                        None => "Unknown",
                                    };
                                    let col_kind_label = match col.remux.as_ref().and_then(|r| r.collection_kind.as_ref()) {
                                        Some(shared::sdks::jellyfin::RemuxCollectionKind::Smart)  => "Smart",
                                        Some(shared::sdks::jellyfin::RemuxCollectionKind::Manual) => "Manual",
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
                shared::sdks::jellyfin::CollectionType::Movies => "movies".to_string(),
                shared::sdks::jellyfin::CollectionType::Tvshows => {
                    "tvshows".to_string()
                }
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
    let mut tags_str = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.tags.as_ref())
            .map(|t| t.join(", "))
            .unwrap_or_default()
    });
    // Selected catalog UUIDs for smart collection filter
    let mut catalog_filter: Signal<Vec<String>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| f.remux.as_ref())
            .and_then(|r| r.collection_catalog_filter.as_ref())
            .map(|ids| ids.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default()
    });
    let mut aio_catalogs: Signal<Vec<AioCatalogInfo>> = use_signal(Vec::new);
    let mut saving = use_signal(|| false);
    let mut err = use_signal(|| Option::<String>::None);

    // Fetch AIO catalogs when kind=smart (for catalog filter checkboxes)
    {
        let client = app_state.client.clone();
        use_effect(move || {
            if col_kind.read().as_str() == "smart" {
                let client = client.clone();
                spawn(async move {
                    if let Ok(catalogs) = client.execute(GetAioCatalogs).await {
                        aio_catalogs.set(catalogs);
                    }
                });
            }
        });
    }

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.client.clone();
        let item_id = existing.as_ref().map(|f| f.id.to_string());
        let name = title.peek().clone();
        let ct = col_type.peek().clone();
        let ck = col_kind.peek().clone();
        let prm = *promoted.peek();
        let filter = catalog_filter.peek().clone();
        let catalog_filter_payload = if ck == "smart" { Some(filter) } else { None };
        let tags_parsed: Vec<String> = tags_str
            .peek()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        saving.set(true);
        err.set(None);
        spawn(async move {
            let result = if let Some(id) = item_id {
                client
                    .execute(PatchItem {
                        item_id: id,
                        payload: PatchItemPayload {
                            name: Some(name),
                            collection_type: Some(ct),
                            collection_kind: Some(ck),
                            collection_catalog_filter: catalog_filter_payload,
                            promoted: Some(prm),
                            tags: Some(tags_parsed),
                        },
                    })
                    .await
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
                    err.set(Some(format!("{e}")));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        p { class: "modal-title",
            if is_edit { "Edit Collection" } else { "New Collection" }
        }

        if let Some(e) = err.read().as_ref() {
            div { class: "alert-error", "{e}" }
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

            if col_kind.read().as_str() == "smart" {
                div { class: "field",
                    label { class: "field-label", "Catalog Filter" }
                    p { class: "field-hint", "Only show items imported from these catalogs. Leave all unchecked for no filter." }
                    {
                        let cats = aio_catalogs.read();
                        let selected = catalog_filter.read();
                        if cats.is_empty() {
                            rsx! { span { class: "field-hint", "Loading catalogs…" } }
                        } else {
                            rsx! {
                                div { style: "display:flex;flex-direction:column;gap:6px",
                                    for cat in cats.iter() {
                                        // Only show catalogs that have a media_id (i.e. have been enabled)
                                        if let Some(mid) = cat.media_id.clone() {
                                            label { style: "display:flex;align-items:center;gap:8px",
                                                input {
                                                    r#type: "checkbox",
                                                    checked: selected.contains(&mid),
                                                    onchange: {
                                                        let cat_id = mid.clone();
                                                        move |e: Event<FormData>| {
                                                            let mut f = catalog_filter.write();
                                                            if e.checked() {
                                                                if !f.contains(&cat_id) {
                                                                    f.push(cat_id.clone());
                                                                }
                                                            } else {
                                                                f.retain(|x| x != &cat_id);
                                                            }
                                                        }
                                                    },
                                                }
                                                span { "{cat.name}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "col-tags", "Tags" }
                input {
                    id: "col-tags",
                    r#type: "text",
                    class: "field-input",
                    placeholder: "action, family, kids",
                    value: "{tags_str}",
                    oninput: move |e| tags_str.set(e.value()),
                }
                p { class: "field-hint", "Comma-separated. Used for per-user content filtering." }
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

// ── Users page ──────────────────────────────────────────────────────

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

// ── IPTV page ─────────────────────────────────────────────────────

#[component]
fn IptvPage(app_state: AppState) -> Element {
    // "sources" or "channels"
    let mut active_tab = use_signal(|| "sources".to_string());

    rsx! {
        IptvSourcesTab { app_state: app_state.clone(), active: active_tab.read().as_str() == "sources" }
        IptvChannelsTab { app_state: app_state.clone(), active: active_tab.read().as_str() == "channels" }
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

// ── IPTV Sources tab ──────────────────────────────────────────────

#[component]
fn IptvSourcesTab(app_state: AppState, active: bool) -> Element {
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
                (Err(e), _) | (_, Err(e)) => error.set(Some(format!("Failed to load: {e}"))),
            }
            loading.set(false);
        });
    });

    if !active {
        return rsx! { div { style: "display:none" } };
    }

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
        // ── Channel sources card ───────────────────────────────────
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
                                                Err(e) => ch_save_error.set(Some(e.to_string())),
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

        // ── EPG sources card ───────────────────────────────────────
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
                                                        epg_form_url.set(epg_clone.url.clone());
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

                                        if url.is_empty() {
                                            epg_save_error.set(Some("URL is required".into()));
                                            return;
                                        }

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
                                                Err(e) => epg_save_error.set(Some(e.to_string())),
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

// ── IPTV Channels tab ─────────────────────────────────────────────

const PAGE_SIZE: u32 = 50;

#[component]
fn IptvChannelsTab(app_state: AppState, active: bool) -> Element {
    let mut channels: Signal<Vec<ChannelEditorItem>> = use_signal(Vec::new);
    let mut total: Signal<usize> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut page = use_signal(|| 0_u32);
    // committed search (triggers fetch); typed search (live input)
    let mut search_committed = use_signal(String::new);
    let mut search_input = use_signal(String::new);
    let mut bulk_working = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let p = *page.read();
        let s = search_committed.read().clone();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client.execute(GetIptvChannels {
                limit: PAGE_SIZE,
                offset: p * PAGE_SIZE,
                search: s,
            }).await {
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

    if !active {
        return rsx! { div { style: "display:none" } };
    }

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
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s }).await {
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
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s }).await {
                                        total.set(r.total_record_count);
                                        channels.set(r.items);
                                    }
                                    loading.set(false);
                                });
                            }
                        },
                        if *bulk_working.read() { "Working…" } else { "Disable All" }
                    }
                    // Search
                    input {
                        class: "form-input",
                        style: "height:32px;font-size:.8rem;width:220px",
                        placeholder: "Search channels…",
                        value: "{search_input.read()}",
                        oninput: move |e| search_input.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter { do_search(); }
                        },
                    }
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        onclick: move |_| do_search(),
                        "Search"
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
                        if total_v == 0 && search_committed.read().is_empty() {
                            "No channels yet. Run a refresh after adding channel sources."
                        } else {
                            "No channels match your search."
                        }
                    }
                } else {
                    div { class: "data-table-container",
                        // Column header
                        div { class: "flex items-center px-3 py-1 border-b border-[var(--border)]",
                            style: "font-size:.72rem;opacity:.5;font-weight:600;gap:8px",
                            div { style: "width:32px", "On" }
                            div { style: "width:64px", "Order" }
                            div { class: "flex-1", "Name / Display Name" }
                            div { style: "width:48px;text-align:right", "Ch#" }
                        }
                        div { class: "row-list",
                            for ch in channels.read().clone() {
                                {
                                    let id = ch.id.clone();
                                    let client1 = app_state.client.clone();
                                    let client2 = app_state.client.clone();
                                    let client3 = app_state.client.clone();
                                    let sort_val = ch.sort_order.map(|n| n.to_string()).unwrap_or_default();
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
                                            input {
                                                class: "form-input",
                                                r#type: "number",
                                                style: "width:64px;height:28px;font-size:.8rem;padding:2px 6px;flex-shrink:0",
                                                value: "{sort_val}",
                                                placeholder: "–",
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
                                            div { class: "flex-1 min-w-0",
                                                div { style: "font-size:.82rem;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis",
                                                    "{ch.name}"
                                                }
                                                input {
                                                    class: "form-input",
                                                    style: "height:24px;font-size:.75rem;padding:2px 6px;margin-top:2px;width:100%",
                                                    value: "{name_val}",
                                                    placeholder: "Custom display name…",
                                                    onchange: move |e| {
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
                                                    },
                                                }
                                            }
                                            div { style: "width:48px;font-size:.8rem;opacity:.5;text-align:right;flex-shrink:0",
                                                if let Some(n) = ch.channel_number { "{n}" }
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
    }
}

// ── Imports page ───────────────────────────────────────────────────

#[component]
fn ImportsPage(app_state: AppState) -> Element {
    let mut catalogs: Signal<Vec<AioCatalogInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut tasks_list: Signal<Vec<TaskInfo>> = use_signal(Vec::new);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            let (cats_result, tasks_result) = futures::join!(
                client.execute(GetAioCatalogs),
                client.execute(GetScheduledTasks {
                    is_hidden: Some(false)
                }),
            );
            match cats_result {
                Ok(cats) => {
                    catalogs.set(cats);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load catalogs: {e}"))),
            }
            if let Ok(t) = tasks_result {
                tasks_list.set(t);
            }
            loading.set(false);
        });
    });

    let import_task = move || {
        tasks_list
            .read()
            .iter()
            .find(|t| t.key.as_deref() == Some("CatalogImport"))
            .cloned()
    };

    let run_import_client = app_state.client.clone();

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "AIO Catalogs" }
                {
                    let task = import_task();
                    let is_running = task.as_ref().and_then(|t| t.state.as_deref()) == Some("Running");
                    let task_id = task.map(|t| t.id);
                    rsx! {
                        button {
                            class: "btn btn-primary",
                            style: "height:32px;font-size:.68rem",
                            disabled: is_running,
                            onclick: move |_| {
                                if let Some(id) = task_id.clone() {
                                    let c = run_import_client.clone();
                                    spawn(async move {
                                        let _ = c.execute(StartTask { task_id: id }).await;
                                    });
                                }
                            },
                            if is_running { "Importing…" } else { "Run Import" }
                        }
                    }
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(e) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{e}" }
                } else if catalogs.read().is_empty() {
                    div { class: "empty-state", "No AIO catalogs found. Check your AIO URL in Settings." }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for cat in catalogs.read().clone() {
                                {
                                    let client = app_state.client.clone();
                                    let cat_aio_id = cat.aio_id.clone();
                                    let cat_name = cat.name.clone();
                                    let mut local_enabled = use_signal(|| cat.enabled.unwrap_or(false));
                                    let max_items_str = cat.max_items.unwrap_or(250).to_string();
                                    let mut local_max = use_signal(|| max_items_str.clone());
                                    // Per-catalog task state
                                    let task_id_opt = cat.media_id.clone()
                                        .map(|id| format!("catalogimport:{}", id));
                                    let cat_task = task_id_opt.as_ref().and_then(|tid|
                                        tasks_list.read().iter().find(|t| &t.id == tid).cloned()
                                    );
                                    let is_importing = cat_task.as_ref()
                                        .and_then(|t| t.state.as_deref()) == Some("Running");
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]", key: "{cat_aio_id}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "catalog-name", "{cat_name}" }
                                                div { class: "catalog-meta",
                                                    span { class: "session-client-badge", "{cat_aio_id}" }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-3",
                                                input {
                                                    r#type: "number",
                                                    class: "field-input",
                                                    style: "width:90px;height:30px;font-size:.75rem",
                                                    placeholder: "Max items",
                                                    value: "{local_max}",
                                                    oninput: move |e| local_max.set(e.value()),
                                                    onchange: {
                                                        let c = client.clone();
                                                        let aio_id = cat_aio_id.clone();
                                                        let name = cat_name.clone();
                                                        move |_| {
                                                            let enabled = *local_enabled.peek();
                                                            let max = local_max.peek().parse::<i64>().ok();
                                                            let c = c.clone();
                                                            let aio_id = aio_id.clone();
                                                            let name = name.clone();
                                                            spawn(async move {
                                                                let _ = c.execute(UpdateCatalogSettings {
                                                                    aio_id,
                                                                    payload: UpdateCatalogSettingsPayload {
                                                                        enabled,
                                                                        max_items: max,
                                                                        name: Some(name),
                                                                    },
                                                                }).await;
                                                            });
                                                        }
                                                    },
                                                }
                                                if let Some(tid) = task_id_opt.clone() {
                                                    if *local_enabled.read() {
                                                        if is_importing {
                                                            button {
                                                                class: "btn btn-danger",
                                                                style: "height:30px;font-size:.68rem",
                                                                onclick: {
                                                                    let c = client.clone();
                                                                    let tid = tid.clone();
                                                                    move |_| {
                                                                        let c = c.clone();
                                                                        let tid = tid.clone();
                                                                        spawn(async move {
                                                                            let _ = c.execute(StopTask { task_id: tid }).await;
                                                                        });
                                                                    }
                                                                },
                                                                "Stop"
                                                            }
                                                        } else {
                                                            button {
                                                                class: "btn btn-secondary",
                                                                style: "height:30px;font-size:.68rem",
                                                                onclick: {
                                                                    let c = client.clone();
                                                                    let tid = tid.clone();
                                                                    move |_| {
                                                                        let c = c.clone();
                                                                        let tid = tid.clone();
                                                                        spawn(async move {
                                                                            let _ = c.execute(StartTask { task_id: tid }).await;
                                                                        });
                                                                    }
                                                                },
                                                                "Import"
                                                            }
                                                        }
                                                    }
                                                }
                                                label { class: "toggle m-0",
                                                    input {
                                                        r#type: "checkbox",
                                                        checked: *local_enabled.read(),
                                                        onchange: {
                                                            let c = client.clone();
                                                            let aio_id = cat_aio_id.clone();
                                                            let name = cat_name.clone();
                                                            move |e: Event<FormData>| {
                                                                let new_enabled = e.checked();
                                                                local_enabled.set(new_enabled);
                                                                let max = local_max.peek().parse::<i64>().ok();
                                                                let c = c.clone();
                                                                let aio_id = aio_id.clone();
                                                                let name = name.clone();
                                                                spawn(async move {
                                                                    let _ = c.execute(UpdateCatalogSettings {
                                                                        aio_id,
                                                                        payload: UpdateCatalogSettingsPayload {
                                                                            enabled: new_enabled,
                                                                            max_items: max,
                                                                            name: Some(name),
                                                                        },
                                                                    }).await;
                                                                });
                                                            }
                                                        },
                                                    }
                                                    span { class: "toggle-track" }
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

        saving.set(true);
        err.set(None);
        spawn(async move {
            let result: Result<(), shared::sdks::ClientError> = async {
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
                    // Update admin flag
                    let mut policy = user.policy.clone();
                    policy.is_administrator = admin;
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
                    // Set admin flag if needed
                    if admin {
                        let mut policy = new_user.policy.clone();
                        policy.is_administrator = true;
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
                    err.set(Some(format!("{e}")));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        p { class: "modal-title",
            if is_edit { "Edit User" } else { "New User" }
        }

        if let Some(e) = err.read().as_ref() {
            div { class: "alert-error", "{e}" }
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

// ── Settings page ───────────────────────────────────────────────────

#[component]
fn SettingsPage(app_state: AppState) -> Element {
    rsx! { ServerSettingsCard { app_state } }
}

#[component]
fn ServerSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut server_name = use_signal(String::new);
    let mut aio_url = use_signal(String::new);
    let mut catalog_max_items = use_signal(|| 100_i64);
    let mut p2p_enabled = use_signal(|| true);
    let mut p2p_upload_speed = use_signal(|| 0_i64);
    let mut p2p_download_speed = use_signal(|| 0_i64);
    let mut filter_digital_release = use_signal(|| true);
    let mut digital_release_buffer = use_signal(|| 0_i64);
    let mut subtitle_languages = use_signal(String::new);
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
                    aio_url.set(cfg.aio_url.clone().unwrap_or_default());
                    catalog_max_items.set(cfg.catalog_max_items.unwrap_or(100));
                    p2p_enabled.set(cfg.p2p_enabled.unwrap_or(true));
                    p2p_upload_speed.set(cfg.p2p_upload_speed_kbps.unwrap_or(0));
                    p2p_download_speed.set(cfg.p2p_download_speed_kbps.unwrap_or(0));
                    filter_digital_release.set(cfg.filter_by_digital_release_date);
                    digital_release_buffer.set(cfg.digital_release_buffer_days);
                    subtitle_languages.set(
                        cfg.subtitle_languages
                            .as_deref()
                            .map(|v| v.join(", "))
                            .unwrap_or_default(),
                    );
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
        let name = server_name.peek().clone();
        let url = aio_url.peek().clone();
        let max = *catalog_max_items.peek();
        let p2p_on = *p2p_enabled.peek();
        let upload = *p2p_upload_speed.peek();
        let download = *p2p_download_speed.peek();
        let filter_dr = *filter_digital_release.peek();
        let dr_buffer = *digital_release_buffer.peek();
        let sub_langs_str = subtitle_languages.peek().clone();

        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        cfg.server_name = Some(name);
        cfg.aio_url = Some(url);
        cfg.catalog_max_items = Some(max);
        cfg.p2p_enabled = Some(p2p_on);
        cfg.p2p_upload_speed_kbps = Some(upload);
        cfg.p2p_download_speed_kbps = Some(download);
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
                Err(e) => error.set(Some(format!("Failed to save: {e}"))),
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
                    if let Some(err) = error.read().as_ref() {
                        div { class: "alert-error", "{err}" }
                    }
                    if *saved.read() {
                        div { class: "alert-success", "Settings saved." }
                    }

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
                            label { class: "field-label", r#for: "s-aio", "AIO URL" }
                            input {
                                id: "s-aio",
                                r#type: "url",
                                class: "field-input",
                                placeholder: "http://192.168.1.x:5000",
                                value: "{aio_url}",
                                oninput: move |e| aio_url.set(e.value()),
                            }
                            p { class: "field-hint",
                                "Base URL of the AIO media backend."
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
                                    checked: *p2p_enabled.read(),
                                    oninput: move |e| p2p_enabled.set(e.checked()),
                                }
                                " Enable P2P Streams"
                            }
                            p { class: "field-hint",
                                "Allow torrent/magnet streams from AIO sources."
                            }
                        }

                        if *p2p_enabled.read() {
                            div { class: "field",
                                label { class: "field-label", r#for: "s-p2p-up", "Upload Speed Limit (KB/s)" }
                                input {
                                    id: "s-p2p-up",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_upload_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() {
                                            p2p_upload_speed.set(n);
                                        }
                                    },
                                }
                                p { class: "field-hint",
                                    "0 = no uploading (seeding disabled). Set a positive value to allow uploading."
                                }
                            }

                            div { class: "field",
                                label { class: "field-label", r#for: "s-p2p-down", "Download Speed Limit (KB/s)" }
                                input {
                                    id: "s-p2p-down",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_download_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() {
                                            p2p_download_speed.set(n);
                                        }
                                    },
                                }
                                p { class: "field-hint",
                                    "0 = unlimited."
                                }
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

// ── Branding page ────────────────────────────────────────────────────

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
                Err(e) => error.set(Some(format!("Failed to save: {e}"))),
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
                    if let Some(err) = error.read().as_ref() {
                        div { class: "alert-error", "{err}" }
                    }
                    if *saved.read() {
                        div { class: "alert-success", "Branding saved." }
                    }

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

// ── Setup wizard ────────────────────────────────────────────────────

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
    let mut aio_url = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);

    // Pre-fill from current startup config (in case the wizard was partially run)
    use_effect(move || {
        let origin = get_origin();
        spawn(async move {
            if let Ok(c) = shared::sdks::jellyfin::client(&origin) {
                if let Ok(cfg) = c.execute(GetStartupConfiguration::default()).await {
                    if let Some(name) = cfg.server_name.filter(|s| !s.is_empty()) {
                        server_name.set(name);
                    }
                    if let Some(url) = cfg.aio_url.filter(|s| !s.is_empty()) {
                        aio_url.set(url);
                    }
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

                        // ── Step 0: server info ────────────────────
                        0 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = server_name.peek().clone();
                                    let url  = aio_url.peek().clone();
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match shared::sdks::jellyfin::client(&origin) {
                                            Ok(c) => match c.execute(PostStartupConfiguration {
                                                payload: StartupConfiguration {
                                                    server_name: Some(name),
                                                    aio_url: Some(url),
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
                                    "Give your server a name and point it at the AIO backend."
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
                                    label { class: "field-label", r#for: "w-aio", "AIO URL" }
                                    input {
                                        id: "w-aio",
                                        r#type: "url",
                                        class: "field-input",
                                        placeholder: "http://192.168.1.x:5000",
                                        required: true,
                                        value: "{aio_url}",
                                        oninput: move |e| aio_url.set(e.value()),
                                    }
                                    p { class: "field-hint",
                                        "Base URL of the AIO media backend (no trailing slash)."
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

                        // ── Step 1: admin account ──────────────────
                        1 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = username.peek().clone();
                                    let pw   = password.peek().clone();
                                    let pw2  = password2.peek().clone();
                                    if name.is_empty() {
                                        error.set(Some("Username is required".into()));
                                        return;
                                    }
                                    if pw != pw2 {
                                        error.set(Some("Passwords do not match".into()));
                                        return;
                                    }
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match shared::sdks::jellyfin::client(&origin) {
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

                        // ── Step 2: finish ─────────────────────────
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
                                                if let Ok(c) = shared::sdks::jellyfin::client(&origin) {
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

// ── Logs page ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct LogLine {
    level: String,
    message: String,
    target: String,
    timestamp: String,
}

#[component]
fn LogsPage(app_state: AppState) -> Element {
    let mut logs: Signal<std::collections::VecDeque<LogLine>> =
        use_signal(std::collections::VecDeque::new);
    let mut paused = use_signal(|| false);
    let mut log_level = use_signal(|| "info".to_string());
    let mut level_error: Signal<Option<String>> = use_signal(|| None);

    let token = app_state.server.access_token.clone();
    let client = app_state.client.clone();

    // Stream log lines via SSE. The coroutine is cancelled on unmount.
    let token_for_coroutine = token.clone();
    use_coroutine(move |_rx: UnboundedReceiver<()>| {
        let token = token_for_coroutine.clone();
        async move {
            let url = format!("/logs/stream?token={token}");
            let mut es = match EventSource::new(&url) {
                Ok(es) => es,
                Err(_) => return,
            };
            let mut stream = match es.subscribe("message") {
                Ok(s) => s,
                Err(_) => return,
            };
            while let Some(Ok((_, event))) = stream.next().await {
                if *paused.peek() {
                    continue;
                }
                if let Some(data) = event.data().as_string() {
                    if let Ok(line) = serde_json::from_str::<LogLine>(&data) {
                        let mut w = logs.write();
                        if w.len() >= 500 {
                            w.pop_front();
                        }
                        w.push_back(line);
                    }
                }
            }
            es.close();
        }
    });

    // Auto-scroll to bottom whenever a new log line arrives
    use_effect(move || {
        let len = logs.read().len();
        if len > 0 {
            if let Some(win) = web_sys::window() {
                if let Some(doc) = win.document() {
                    if let Some(el) = doc.get_element_by_id("log-scroll") {
                        let _ = el.scroll_into_view_with_bool(false);
                    }
                }
            }
        }
    });

    let on_level_change = move |evt: Event<FormData>| {
        let new_level = evt.value().to_lowercase();
        log_level.set(new_level.clone());
        let client = client.clone();
        level_error.set(None);
        spawn(async move {
            if let Err(_) = client.execute(SetLogLevel { level: new_level }).await {
                level_error.set(Some("Failed to update log level".into()));
            }
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Server Logs" }
                div { style: "display:flex;gap:8px;align-items:center;margin-left:auto",

                    // Level selector
                    select {
                        class: "form-select",
                        style: "width:auto",
                        value: "{log_level}",
                        onchange: on_level_change,
                        option { value: "trace", "Trace" }
                        option { value: "debug", "Debug" }
                        option { value: "info", selected: true, "Info" }
                        option { value: "warn", "Warn" }
                        option { value: "error", "Error" }
                    }

                    // Pause / Resume
                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| {
                            let p = *paused.read();
                            paused.set(!p);
                        },
                        if *paused.read() { "▶ Resume" } else { "⏸ Pause" }
                    }

                    // Clear
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| logs.write().clear(),
                        "Clear"
                    }
                }
            }

            if let Some(err) = level_error.read().as_ref() {
                div { class: "alert alert-error", style: "margin:8px 16px 0",
                    "{err}"
                }
            }

            div {
                id: "log-scroll",
                style: "height:600px;overflow:auto;padding:12px;font-family:monospace;font-size:0.8rem;background:var(--color-bg, #0d0d0d)",
                for line in logs.read().iter() {
                    div {
                        style: "display:flex;gap:8px;margin-bottom:2px;white-space:nowrap",
                        span { style: "color:#666;flex-shrink:0", "{line.timestamp}" }
                        span {
                            style: "flex-shrink:0;font-weight:600;{level_color(&line.level)}",
                            "[{line.level}]"
                        }
                        span { style: "color:#888;flex-shrink:0", "{line.target}" }
                        span { style: "color:#ddd", "{line.message}" }
                    }
                }
                // Anchor element used for auto-scroll
                div { id: "log-scroll" }
            }
        }
    }
}

fn level_color(level: &str) -> &'static str {
    match level.to_uppercase().as_str() {
        "TRACE" => "color:#9ca3af",
        "DEBUG" => "color:#60a5fa",
        "INFO" => "color:#34d399",
        "WARN" => "color:#fbbf24",
        "ERROR" => "color:#f87171",
        _ => "color:#e5e7eb",
    }
}
