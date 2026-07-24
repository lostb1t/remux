use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::{fmt_datetime, fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    ActivityLogEntry, GetActivityLog, GetSessions, SessionInfoDto,
};

#[component]
pub fn SessionsCard(app_state: AppState) -> Element {
    let mut sessions: Signal<Vec<SessionInfoDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        loading.set(true);
        let client = app_state.clone();
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
                Err(e) => error.set(Some(format!("Failed to load sessions: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Active Devices",
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                ErrorAlert { message: err.clone() }
            } else if sessions.read().is_empty() {
                EmptyState { message: "No active devices in the last 16 minutes" }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for session in sessions.read().iter() {
                            div {
                                class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                key: "{session.id.as_deref().unwrap_or(\"\")}",
                                div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                    div { class: "session-name",
                                        "{session.device_name.as_deref().unwrap_or(\"Unknown device\")}"
                                    }
                                    if let Some(item) = &session.now_playing_item {
                                        div { class: "session-playing text-xs text-[var(--text-dim)] mt-0.5",
                                            "▶ {item.name.as_deref().unwrap_or(\"\")}"
                                        }
                                    }
                                }
                                div { class: "shrink-0 px-3 py-[10px]",
                                    if let Some(user) = &session.user_name {
                                        div { class: "session-user", "{user}" }
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

#[component]
pub fn ActivityCard(app_state: AppState) -> Element {
    let mut activity_items: Signal<Vec<ActivityLogEntry>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);

    use_effect(move || {
        loading.set(true);
        let client = app_state.clone();
        spawn(async move {
            if let Ok(result) = client
                .execute(GetActivityLog {
                    start_index: Some(0),
                    limit: Some(50),
                })
                .await
            {
                activity_items.set(result.items);
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Admin Activity Log",
            if *loading.read() {
                LoadingText {}
            } else if activity_items.read().is_empty() {
                EmptyState { message: "No admin actions recorded yet" }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for entry in activity_items.read().iter() {
                            div {
                                class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                key: "{entry.id.as_deref().unwrap_or(\"\")}",
                                div { class: "shrink-0 px-3 py-[8px] font-mono text-xs text-[var(--text-dim)] w-40",
                                    if let Some(ts) = entry.date {
                                        "{fmt_datetime(ts)}"
                                    }
                                }
                                div { class: "shrink-0 px-3 py-[8px] text-xs text-[var(--text-dim)] w-32",
                                    "{entry.remux.as_ref().and_then(|r| r.user_name.as_deref()).unwrap_or(\"\")}"
                                }
                                div { class: "shrink-0 px-3 py-[8px] text-xs font-medium w-40",
                                    "{entry.name.as_deref().unwrap_or(\"\")}"
                                }
                                div { class: "flex-1 min-w-0 px-3 py-[8px] text-xs text-[var(--text-dim)]",
                                    if let Some(target) = entry.remux.as_ref().and_then(|r| r.target_user_name.as_deref()) {
                                        "user: {target}"
                                    }
                                    if let Some(dev) = entry.remux.as_ref().and_then(|r| r.device_name.as_deref()) {
                                        span { style: "margin-left:8px", "device: {dev}" }
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
