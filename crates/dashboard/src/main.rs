use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use shared::sdks::jellyfin::{AuthenticateUserByName, GetScheduledTasks, GetSessions, JellyfinAuth, PublicSystemInfo, SessionInfoDto, StartTask, StopTask, TaskInfo};
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
    let mut logged_in = use_signal(|| get_stored_server().is_some());

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: THEME_CSS }
        if *logged_in.read() {
            Dashboard { on_logout: move |_| logged_in.set(false) }
        } else {
            Login { on_login: move |_| logged_in.set(true) }
        }
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
        Page::Overview => "Overview",
        Page::Devices => "Devices",
        Page::Tasks => "Scheduled Tasks",
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
                        label: "Devices",
                        active: *current_page.read() == Page::Devices,
                        on_click: move |_| { current_page.set(Page::Devices); sidebar_open.set(false); },
                    }
                    NavItem {
                        label: "Tasks",
                        active: *current_page.read() == Page::Tasks,
                        on_click: move |_| { current_page.set(Page::Tasks); sidebar_open.set(false); },
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
                    }}
                }
            }
        }
    }
}
