use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::{fmt_datetime, fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    ActivityLogEntry, GetActivityLog, GetSessions, SessionInfoDto,
};

const PAGE_SIZE: i64 = 25;

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
    let mut page: Signal<i64> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut search_input: Signal<String> = use_signal(String::new);

    use_effect(move || {
        let page_v = *page.read();
        let offset = page_v * PAGE_SIZE;
        let search = search_input
            .read()
            .clone();
        loading.set(true);
        let client = app_state.clone();
        spawn(async move {
            let search_term = if search.is_empty() {
                None
            } else {
                Some(search)
            };
            match client
                .execute(GetActivityLog {
                    start_index: Some(offset),
                    limit: Some(PAGE_SIZE),
                    search_term,
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
    let page_v = *page.read();
    let total_pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    rsx! {
        Card { title: "Admin Activity Log", tight: true,
            div { class: "device-search",
                input {
                    r#type: "text",
                    class: "input",
                    placeholder: "Filter by user, action, or device…",
                    value: "{search_input.read()}",
                    oninput: move |evt| {
                        search_input.set(evt.value());
                        page.set(0);
                    },
                }
                if !search_input.read().is_empty() {
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;padding:0 8px;font-size:.75rem",
                        onclick: move |_| {
                            search_input.set(String::new());
                            page.set(0);
                        },
                        "×"
                    }
                }
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                ErrorAlert { message: err.clone() }
            } else if activity_items.read().is_empty() {
                EmptyState { message: if search_input.read().is_empty() { "No admin actions recorded yet" } else { "No results match your filter" } }
            } else {
                div { class: "data-table-container",
                    div { style: "overflow-x:auto;-webkit-overflow-scrolling:touch",
                        div { class: "row-list", style: "min-width:520px;width:100%",
                            div { class: "activity-col-header",
                                span { class: "activity-col-date", "Date" }
                                span { class: "activity-col-admin", "Admin" }
                                span { class: "activity-col-action", "Action" }
                                span { class: "activity-col-target", "Target" }
                            }
                            for entry in activity_items.read().iter() {
                                {
                                    let action = entry.name.as_deref().unwrap_or("");
                                    let action_color = if action.to_lowercase().contains("revoke") {
                                        "color:var(--error)"
                                    } else {
                                        ""
                                    };
                                    rsx! {
                                div {
                                    class: "activity-row flex items-center border-b border-[var(--border)] hover:bg-[var(--hover-overlay)]",
                                    key: "{entry.id.as_deref().unwrap_or(\"\")}",
                                    div { class: "activity-col-date font-mono text-xs text-[var(--text-dim)]",
                                        if let Some(ts) = entry.date {
                                            "{fmt_datetime(ts)}"
                                        }
                                    }
                                    div { class: "activity-col-admin font-mono text-xs text-[var(--text-dim)]",
                                        "{entry.remux.as_ref().and_then(|r| r.user_name.as_deref()).unwrap_or(\"\")}"
                                    }
                                    div {
                                        class: "activity-col-action text-xs font-semibold",
                                        style: "{action_color}",
                                        "{action}"
                                    }
                                    div { class: "activity-col-target flex items-center gap-2",
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
                        }
                    }

                    if total_pages > 1 {
                        div { class: "pagination-bar",
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.75rem",
                                disabled: page_v == 0,
                                onclick: move |_| page.set((page_v - 1).max(0)),
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
