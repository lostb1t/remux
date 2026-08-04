use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::{fmt_datetime, fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    ActivityLogEntry, GetActivityLog, GetSessions, SessionInfoDto,
};

const ACTIVITY_PAGE_SIZE: i64 = 25;

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
        Card { title: "Active Devices", tight: true,
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                ErrorAlert { message: err.clone() }
            } else if sessions.read().is_empty() {
                EmptyState { message: "No active devices in the last 16 minutes" }
            } else {
                div { class: "data-table-container",
                    div { style: "overflow-x:auto;-webkit-overflow-scrolling:touch",
                        div { class: "row-list", style: "min-width:480px",
                            for session in sessions.read().iter() {
                                div {
                                    class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--hover-overlay)]",
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
}

#[component]
pub fn ActivityCard(app_state: AppState) -> Element {
    let mut activity_items: Signal<Vec<ActivityLogEntry>> = use_signal(Vec::new);
    let mut total_count: Signal<i64> = use_signal(|| 0);
    let mut start_index: Signal<i64> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let offset = *start_index.read();
        loading.set(true);
        let client = app_state.clone();
        spawn(async move {
            match client
                .execute(GetActivityLog {
                    start_index: Some(offset),
                    limit: Some(ACTIVITY_PAGE_SIZE),
                })
                .await
            {
                Ok(result) => {
                    total_count.set(result.total_record_count);
                    activity_items.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load activity log: {e}"))),
            }
            loading.set(false);
        });
    });

    let total = *total_count.read();
    let offset = *start_index.read();
    let has_prev = offset > 0;
    let has_next = offset + ACTIVITY_PAGE_SIZE < total;

    rsx! {
        Card { title: "Admin Activity Log", tight: true,
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                ErrorAlert { message: err.clone() }
            } else if activity_items.read().is_empty() {
                EmptyState { message: "No admin actions recorded yet" }
            } else {
                div { class: "data-table-container",
                    div { style: "overflow-x:auto;-webkit-overflow-scrolling:touch",
                        div { class: "row-list", style: "min-width:520px",
                            for entry in activity_items.read().iter() {
                                div {
                                    class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--hover-overlay)]",
                                    key: "{entry.id.as_deref().unwrap_or(\"\")}",
                                    div { class: "shrink-0 px-3 py-[8px] font-mono text-xs text-[var(--text-dim)] w-40",
                                        if let Some(ts) = entry.date {
                                            "{fmt_datetime(ts)}"
                                        }
                                    }
                                    div { class: "shrink-0 px-3 py-[8px] font-mono text-xs text-[var(--text-dim)] w-32",
                                        "{entry.remux.as_ref().and_then(|r| r.user_name.as_deref()).unwrap_or(\"\")}"
                                    }
                                    div { class: "shrink-0 px-3 py-[8px] text-xs font-semibold w-40",
                                        "{entry.name.as_deref().unwrap_or(\"\")}"
                                    }
                                    div { class: "flex-1 min-w-0 flex items-center gap-2 px-3 py-[8px]",
                                        if let Some(target) = entry.remux.as_ref().and_then(|r| r.target_user_name.as_deref()) {
                                            span { class: "session-user", "{target}" }
                                        }
                                        if let Some(dev) = entry.remux.as_ref().and_then(|r| r.device_name.as_deref()) {
                                            span { class: "session-client-badge", "{dev}" }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if has_prev || has_next {
                        div { class: "flex items-center justify-between px-3 py-2 border-t border-[var(--border)]",
                            span { class: "text-xs text-[var(--text-dim)]",
                                "{offset + 1}–{(offset + ACTIVITY_PAGE_SIZE).min(total)} of {total}"
                            }
                            div { class: "flex gap-2",
                                button {
                                    class: "btn btn-ghost",
                                    style: "height:28px;font-size:.72rem;padding:0 10px",
                                    disabled: !has_prev,
                                    onclick: move |_| start_index.set((offset - ACTIVITY_PAGE_SIZE).max(0)),
                                    "← Prev"
                                }
                                button {
                                    class: "btn btn-ghost",
                                    style: "height:28px;font-size:.72rem;padding:0 10px",
                                    disabled: !has_next,
                                    onclick: move |_| start_index.set(offset + ACTIVITY_PAGE_SIZE),
                                    "Next →"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
