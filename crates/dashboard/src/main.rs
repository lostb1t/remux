use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use shared::sdks::jellyfin::{
    AioCatalogInfo, AuthenticateUserByName, BaseItemDto, CreateVirtualFolder,
    CreateVirtualFolderPayload, DeleteVirtualFolder, GetAioCatalogs, GetItems,
    GetScheduledTasks, GetSessions, GetSystemConfiguration,
    GetStartupConfiguration, JellyfinAuth, PostStartupComplete,
    PostStartupConfiguration, PostStartupUser, PublicSystemInfo, ServerConfiguration,
    SessionInfoDto, StartTask, StartupConfiguration, StartupUser, StopTask,
    TaskInfo, UpdateSystemConfiguration, UpdateVirtualFolder,
    UpdateVirtualFolderPayload,
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
        let auth = JellyfinAuth::new(&device_id).with_token(server.access_token.clone());
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
    let _ = LocalStorage::set(CREDENTIALS_KEY, &StoredCredentials { servers: vec![server] });
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // None = still checking, Some(true) = wizard needed, Some(false) = normal flow
    let mut wizard_needed: Signal<Option<bool>> = use_signal(|| None);
    let mut logged_in = use_signal(|| get_stored_server().is_some());

    use_effect(move || {
        spawn(async move {
            let origin = get_origin();
            let needed = match shared::sdks::jellyfin::client(&origin) {
                Ok(c) => c.execute(PublicSystemInfo::default()).await
                    .ok()
                    .and_then(|info| info.startup_wizard_completed)
                    .map(|done| !done)  // wizard needed = wizard NOT yet completed
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
                    Dashboard { on_logout: move |_| logged_in.set(false) }
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

            match client.execute(AuthenticateUserByName { username: u, pw: p }).await {
                Ok(result) => {
                    if let (Some(token), Some(user)) = (result.access_token, result.user) {
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
                Ok(info) => { server_info.set(Some(info)); error.set(None); }
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
                    KvRow { label: "Name", value: info.server_name.clone().unwrap_or_default() }
                    KvRow { label: "Version", value: info.version.clone().unwrap_or_default() }
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
            match client.execute(GetSessions { active_within_seconds: Some(960) }).await {
                Ok(s) => { sessions.set(s); error.set(None); }
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
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if sessions.read().is_empty() {
                    div { class: "empty-state", "No active devices in the last 16 minutes" }
                } else {
                    for session in sessions.read().iter() {
                        div { class: "session-row",
                            div {
                                div { class: "session-name",
                                    "{session.device_name.as_deref().unwrap_or(\"Unknown device\")}"
                                }
                                div { class: "session-meta",
                                    span { class: "session-user",
                                        "{session.user_name.as_deref().unwrap_or(\"Unknown\")}"
                                    }
                                    if let Some(client_name) = &session.client {
                                        span { class: "session-client-badge",
                                            "{client_name}"
                                            if let Some(v) = &session.application_version {
                                                " {v}"
                                            }
                                        }
                                    }
                                }
                                if let Some(item) = &session.now_playing_item {
                                    div { class: "session-playing",
                                        "▶ {item.name.as_deref().unwrap_or(\"Unknown\")}"
                                    }
                                }
                            }
                            span { class: "session-time",
                                "{fmt_time(session.last_activity_date)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TasksCard(app_state: AppState, #[props(default = false)] running_only: bool) -> Element {
    let mut tasks: Signal<Vec<TaskInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh: Signal<u32> = use_signal(|| 0);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read(); // re-run effect when refresh increments
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            match client.execute(GetScheduledTasks { is_hidden: Some(false) }).await {
                Ok(t) => { tasks.set(t); error.set(None); }
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
            div { class: "card-body",
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
                                for task in visible {
                                    TaskRow { key: "{task.id}", task }
                                }
                            }
                        } else {
                            rsx! {
                                for task in visible {
                                    TaskPageRow {
                                        key: "{task.id}",
                                        task,
                                        app_state: app_state.clone(),
                                        on_refresh: move |_| {
                                            let v = *refresh.peek() + 1;
                                            refresh.set(v);
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

/// Wraps `TaskRow` with start/stop controls; used on the Tasks page.
#[component]
fn TaskPageRow(task: TaskInfo, app_state: AppState, on_refresh: EventHandler) -> Element {
    let start_id = task.id.clone();
    let stop_id  = task.id.clone();
    let c_start  = app_state.client.clone();
    let c_stop   = app_state.client.clone();

    rsx! {
        TaskRow {
            task,
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
    #[props(optional)] on_start: Option<EventHandler>,
    #[props(optional)] on_stop: Option<EventHandler>,
) -> Element {
    let state = task.state.as_deref().unwrap_or("Idle");
    let is_running = state == "Running";

    // Last result status shown when idle
    let last_status = task.last_execution_result
        .as_ref()
        .and_then(|r| r.status.as_deref())
        .unwrap_or("");

    let display_state = if is_running { state } else { last_status };
    let display_badge = if is_running {
        "task-badge task-badge-running"
    } else {
        match last_status {
            "Completed" => "task-badge task-badge-completed",
            "Failed"    => "task-badge task-badge-failed",
            _           => "task-badge task-badge-idle",
        }
    };

    let has_controls = on_start.is_some() || on_stop.is_some();

    rsx! {
        div { class: "task-row",
            div { style: "min-width:0; flex:1",
                div { class: "task-name", "{task.name}" }
                if let Some(cat) = &task.category {
                    div { class: "task-category", "{cat}" }
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
            div { class: "task-right",
                if !display_state.is_empty() {
                    span { class: "{display_badge}", "{display_state}" }
                }
                if has_controls {
                    div { class: "task-actions",
                        if !is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Run now",
                                onclick: move |_| { if let Some(ref h) = on_start { h.call(()); } },
                                "▶"
                            }
                        }
                        if is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Stop",
                                onclick: move |_| { if let Some(ref h) = on_stop { h.call(()); } },
                                "■"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Page {
    Overview,
    Devices,
    Tasks,
    Collections,
    Settings,
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
fn Dashboard(on_logout: EventHandler) -> Element {
    let server = match get_stored_server() {
        Some(s) => s,
        None => return rsx! { div { "Not logged in" } },
    };

    let app_state = AppState::new(server);
    let mut sidebar_open = use_signal(|| false);
    let mut current_page = use_signal(|| Page::Overview);

    let page_title = match *current_page.read() {
        Page::Overview    => "Overview",
        Page::Devices     => "Devices",
        Page::Tasks       => "Scheduled Tasks",
        Page::Collections => "Collections",
        Page::Settings    => "Settings",
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
                        active: *current_page.read() == Page::Overview,
                        on_click: move |_| { current_page.set(Page::Overview); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Collections",
                        active: *current_page.read() == Page::Collections,
                        on_click: move |_| { current_page.set(Page::Collections); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Devices",
                        active: *current_page.read() == Page::Devices,
                        on_click: move |_| { current_page.set(Page::Devices); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Tasks",
                        active: *current_page.read() == Page::Tasks,
                        on_click: move |_| { current_page.set(Page::Tasks); sidebar_open.set(false); },
                    }

                    NavItem {
                        label: "Settings",
                        active: *current_page.read() == Page::Settings,
                        on_click: move |_| { current_page.set(Page::Settings); sidebar_open.set(false); },
                    }
                }

                div { class: "sidebar-footer",
                    button {
                        class: "btn btn-ghost",
                        style: "width:100%",
                        onclick: move |_| {
                            LocalStorage::delete(CREDENTIALS_KEY);
                            on_logout.call(());
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
                    {match *current_page.read() {
                        Page::Overview => rsx! {
                            ServerInfoCard { app_state: app_state.clone() }
                            SessionsCard { app_state: app_state.clone() }
                            TasksCard { app_state: app_state.clone(), running_only: true }
                        },
                        Page::Devices => rsx! {
                            SessionsCard { app_state: app_state.clone() }
                        },
                        Page::Tasks => rsx! {
                            TasksCard { app_state: app_state.clone() }
                        },
                        Page::Collections => rsx! {
                            CollectionsPage { app_state: app_state.clone() }
                        },
                        Page::Settings => rsx! {
                            SettingsPage { app_state: app_state.clone() }
                        },
                    }}
                }
            }
        }
    }
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
    let mut tasks_list:  Signal<Vec<TaskInfo>>    = use_signal(Vec::new);
    let mut loading   = use_signal(|| true);
    let mut error     = use_signal(|| Option::<String>::None);
    let mut refresh   = use_signal(|| 0_u32);
    let mut form_mode: Signal<Option<FormMode>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.client.clone();
        spawn(async move {
            let (cols_result, tasks_result) = futures::join!(
                client.execute(GetItems {
                    include_item_types: vec!["BoxSet".to_string(), "CollectionFolder".to_string()],
                    recursive: false,
                }),
                client.execute(GetScheduledTasks { is_hidden: Some(false) }),
            );
            match cols_result {
                Ok(result) => { collections.set(result.items); error.set(None); }
                Err(e) => error.set(Some(format!("Failed to load collections: {e}"))),
            }
            if let Ok(t) = tasks_result {
                tasks_list.set(t);
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
            div { class: "card-body",
                if *loading.read() {
                    span { class: "loading-text", "Loading…" }
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if collections.read().is_empty() {
                    div { class: "empty-state", "No collections yet" }
                } else {
                    for col in collections.read().clone() {
                        {
                            let col_edit = col.clone();
                            let col_del  = col.clone();
                            let client_del = app_state.client.clone();
                            let client_import = app_state.client.clone();
                            let col_id_str = col.id.to_string();
                            let task_key = format!("catalog_import:{}", col_id_str);
                            let import_task = tasks_list.read().iter()
                                .find(|t| t.key.as_deref() == Some(&task_key))
                                .cloned();
                            let is_catalog = col.collection_kind.as_deref() == Some("catalog");
                            let name = col.name.clone().unwrap_or_default();
                            let col_type_label = match col.collection_type.as_ref() {
                                Some(ct) => match ct {
                                    shared::sdks::jellyfin::CollectionType::Movies  => "Movies",
                                    shared::sdks::jellyfin::CollectionType::Tvshows => "Shows",
                                    _ => "Unknown",
                                },
                                None => "Unknown",
                            };
                            let col_kind_label = match col.collection_kind.as_deref() {
                                Some("smart")   => "Smart",
                                Some("manual")  => "Manual",
                                Some("catalog") => "Catalog",
                                _ => "",
                            };
                            rsx! {
                                div { class: "catalog-row", key: "{col_id_str}",
                                    div {
                                        div { class: "catalog-name", "{name}" }
                                        div { class: "catalog-meta",
                                            span { class: "session-client-badge", "{col_type_label}" }
                                            if !col_kind_label.is_empty() {
                                                span { class: "session-client-badge", "{col_kind_label}" }
                                            }
                                        }
                                    }
                                    div { class: "catalog-actions",
                                        if is_catalog {
                                            {
                                                let task_id = import_task.as_ref().map(|t| t.id.clone());
                                                let is_running = import_task.as_ref()
                                                    .and_then(|t| t.state.as_deref())
                                                    == Some("Running");
                                                if let Some(tid) = task_id {
                                                    rsx! {
                                                        button {
                                                            class: "btn btn-ghost",
                                                            style: "height:30px;font-size:.68rem;padding:0 10px",
                                                            disabled: is_running,
                                                            onclick: move |_| {
                                                                let id = tid.clone();
                                                                let c  = client_import.clone();
                                                                spawn(async move {
                                                                    let _ = c.execute(StartTask { task_id: id }).await;
                                                                });
                                                            },
                                                            if is_running { "Importing…" } else { "Import" }
                                                        }
                                                    }
                                                } else { rsx! {} }
                                            }
                                        }
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
        FormMode::Create  => None,
    };

    let mut title       = use_signal(|| existing.as_ref().and_then(|f| f.name.clone()).unwrap_or_default());
    let mut promoted    = use_signal(|| false);
    let mut col_type    = use_signal(|| {
        existing.as_ref()
            .and_then(|f| f.collection_type.as_ref())
            .map(|ct| match ct {
                shared::sdks::jellyfin::CollectionType::Movies  => "movies".to_string(),
                shared::sdks::jellyfin::CollectionType::Tvshows => "tvshows".to_string(),
                _ => "movies".to_string(),
            })
            .unwrap_or_else(|| "movies".to_string())
    });
    let mut col_kind    = use_signal(|| {
        existing.as_ref()
            .and_then(|f| f.collection_kind.clone())
            .unwrap_or_else(|| "smart".to_string())
    });
    let mut max_items   = use_signal(|| {
        "250".to_string()
    });
    let mut aio_id      = use_signal(String::new);
    let mut aio_catalogs: Signal<Vec<AioCatalogInfo>> = use_signal(Vec::new);
    let mut saving      = use_signal(|| false);
    let mut err         = use_signal(|| Option::<String>::None);

    // Fetch AIO catalogs when kind=catalog (create mode only)
    {
        let client = app_state.client.clone();
        use_effect(move || {
            if !is_edit && col_kind.read().as_str() == "catalog" {
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
        let ct   = col_type.peek().clone();
        let ck   = col_kind.peek().clone();
        let prm  = *promoted.peek();
        let max  = if ck == "catalog" {
            max_items.peek().parse::<i64>().ok()
        } else {
            None
        };
        let aid  = if ck == "catalog" && item_id.is_none() {
            let v = aio_id.peek().clone();
            if v.is_empty() { None } else { Some(v) }
        } else {
            None
        };
        saving.set(true);
        err.set(None);
        spawn(async move {
            let result = if let Some(id) = item_id {
                client.execute(UpdateVirtualFolder {
                    payload: UpdateVirtualFolderPayload {
                        id,
                        name,
                        collection_type: Some(ct),
                        collection_kind: Some(ck),
                        promoted: Some(prm),
                        collection_max_items: max,
                    },
                }).await
            } else {
                client.execute(CreateVirtualFolder {
                    payload: CreateVirtualFolderPayload {
                        name,
                        collection_type: Some(ct),
                        collection_kind: Some(ck),
                        promoted: Some(prm),
                        collection_max_items: max,
                        aio_id: aid,
                    },
                }).await.map(|_| ())
            };
            match result {
                Ok(_)  => on_done.call(()),
                Err(e) => { err.set(Some(format!("{e}"))); saving.set(false); }
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
                    option { value: "smart",   "Smart"   }
                    option { value: "manual",  "Manual"  }
                    option { value: "catalog", "Catalog" }
                }
            }

            if col_kind.read().as_str() == "catalog" {
                if !is_edit {
                    div { class: "field",
                        label { class: "field-label", r#for: "col-aio", "AIO Catalog" }
                        {
                            let cats = aio_catalogs.read();
                            if cats.is_empty() {
                                rsx! {
                                    select {
                                        id: "col-aio",
                                        class: "select-input",
                                        disabled: true,
                                        option { "Loading catalogs…" }
                                    }
                                }
                            } else {
                                rsx! {
                                    select {
                                        id: "col-aio",
                                        class: "select-input",
                                        value: "{aio_id}",
                                        onchange: move |e| aio_id.set(e.value()),
                                        for cat in cats.iter() {
                                            option { value: "{cat.aio_id}", "{cat.name}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "field",
                    label { class: "field-label", r#for: "col-max", "Max Items" }
                    input {
                        id: "col-max",
                        r#type: "number",
                        class: "field-input",
                        min: "1",
                        placeholder: "250",
                        value: "{max_items}",
                        oninput: move |e| max_items.set(e.value()),
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
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut server_name       = use_signal(String::new);
    let mut aio_url           = use_signal(String::new);
    let mut catalog_max_items = use_signal(|| 100_i64);
    let mut loading = use_signal(|| true);
    let mut saving  = use_signal(|| false);
    let mut error   = use_signal(|| Option::<String>::None);
    let mut saved   = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.client.clone();
        spawn(async move {
            match client.execute(GetSystemConfiguration).await {
                Ok(cfg) => {
                    server_name.set(cfg.server_name.clone().unwrap_or_default());
                    aio_url.set(cfg.aio_url.clone().unwrap_or_default());
                    catalog_max_items.set(cfg.catalog_max_items.unwrap_or(100));
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
        let url  = aio_url.peek().clone();
        let max  = *catalog_max_items.peek();

        let mut cfg = base_cfg.peek().clone().unwrap_or_default();
        cfg.server_name       = Some(name);
        cfg.aio_url           = Some(url);
        cfg.catalog_max_items = Some(max);

        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client.execute(UpdateSystemConfiguration { config: cfg }).await {
                Ok(_)  => saved.set(true),
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
    let mut step      = use_signal(|| 0_u8);
    let mut server_name = use_signal(String::new);
    let mut aio_url   = use_signal(String::new);
    let mut username  = use_signal(String::new);
    let mut password  = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving    = use_signal(|| false);
    let mut error     = use_signal(|| Option::<String>::None);

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
