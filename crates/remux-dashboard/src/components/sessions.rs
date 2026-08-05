use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::{fmt_datetime, fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    ActivityLogEntry, GetActivityLog, GetSessions, SessionInfoDto,
};

const DEFAULT_ACTIVITY_PAGE_SIZE: i64 = 15;

enum PageItem {
    Page(i64),
    Ellipsis,
}

fn paginate(current: i64, total: i64) -> Vec<PageItem> {
    if total <= 7 {
        return (0..total)
            .map(PageItem::Page)
            .collect();
    }
    let mut items = Vec::new();
    items.push(PageItem::Page(0));
    if current > 2 {
        items.push(PageItem::Ellipsis);
    }
    for p in (current - 1).max(1)..=(current + 1).min(total - 2) {
        items.push(PageItem::Page(p));
    }
    if current < total - 3 {
        items.push(PageItem::Ellipsis);
    }
    items.push(PageItem::Page(total - 1));
    items
}

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
    let mut page_size: Signal<i64> = use_signal(|| DEFAULT_ACTIVITY_PAGE_SIZE);
    let mut search_input: Signal<String> = use_signal(String::new);

    use_effect(move || {
        let offset = *start_index.read();
        let limit = *page_size.read();
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
                    limit: Some(limit),
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
    let offset = *start_index.read();
    let ps = *page_size.read();
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
                        start_index.set(0);
                    },
                }
                if !search_input.read().is_empty() {
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;padding:0 8px;font-size:.75rem",
                        onclick: move |_| {
                            search_input.set(String::new());
                            start_index.set(0);
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

                    div { class: "pagination-bar",
                        div { class: "flex items-center gap-1",
                            span { class: "pagination-summary",
                                "{offset + 1}–{(offset + ps).min(total)} of {total}"
                            }
                            span { class: "pagination-size-label", "·" }
                            select {
                                class: "pagination-size-select",
                                value: "{ps}",
                                onchange: move |evt| {
                                    if let Ok(v) = evt.value().parse::<i64>() {
                                        page_size.set(v);
                                        start_index.set(0);
                                    }
                                },
                                option { value: "15", selected: ps == 15, "15" }
                                option { value: "25", selected: ps == 25, "25" }
                                option { value: "50", selected: ps == 50, "50" }
                                option { value: "100", selected: ps == 100, "100" }
                            }
                            span { class: "pagination-size-label", "/ page" }
                        }
                        div { class: "flex items-center gap-2",
                            {
                                let total_pages = ((total as f64) / (ps as f64)).ceil() as i64;
                                let current_page = offset / ps;
                                if total_pages > 1 {
                                    let items = paginate(current_page, total_pages);
                                    rsx! {
                                        if current_page > 0 {
                                            button {
                                                class: "pagination-page",
                                                onclick: move |_| start_index.set((offset - ps).max(0)),
                                                "‹"
                                            }
                                        }
                                        for (i, item) in items.iter().enumerate() {
                                            match item {
                                                PageItem::Page(p) => {
                                                    let p = *p;
                                                    rsx! {
                                                        button {
                                                            key: "p{p}",
                                                            class: if p == current_page { "pagination-page active" } else { "pagination-page" },
                                                            disabled: p == current_page,
                                                            onclick: move |_| start_index.set(p * ps),
                                                            "{p + 1}"
                                                        }
                                                    }
                                                }
                                                PageItem::Ellipsis => rsx! {
                                                    span { key: "e{i}", class: "pagination-ellipsis", "…" }
                                                },
                                            }
                                        }
                                        if current_page < total_pages - 1 {
                                            button {
                                                class: "pagination-page",
                                                onclick: move |_| start_index.set(offset + ps),
                                                "›"
                                            }
                                        }
                                    }
                                } else {
                                    rsx! {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
