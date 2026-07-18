use crate::{
    components::{Card, EmptyState, LoadingText},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{ActivityLogEntry, GetActivityLog};

const PAGE_SIZE: i64 = 50;

/// Map a Jellyfin `LogLevel` severity to an inline color, so errors/warnings
/// stand out in the table. Pure, so it is unit-tested.
fn severity_color(severity: &str) -> &'static str {
    match severity {
        "Error" | "Critical" => "var(--error)",
        "Warning" => "var(--warning, #f59e0b)",
        _ => "var(--text-muted)",
    }
}

/// Activity log: a paged, newest-first audit trail of server events (logins,
/// playback, task failures, user changes). Backed by `/System/ActivityLog/Entries`.
#[component]
pub fn ActivityLogPage(app_state: AppState) -> Element {
    let mut entries: Signal<Vec<ActivityLogEntry>> = use_signal(Vec::new);
    let mut total = use_signal(|| 0_i64);
    let mut loading = use_signal(|| true);
    let mut loading_more = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);

    // Initial load.
    let app_state_effect = app_state.clone();
    use_effect(move || {
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetActivityLog {
                    start_index: Some(0),
                    limit: Some(PAGE_SIZE),
                    has_user_id: None,
                })
                .await
            {
                Ok(result) => {
                    total.set(result.total_record_count);
                    entries.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load activity log: {e}"))),
            }
            loading.set(false);
        });
    });

    let has_more = (entries
        .read()
        .len() as i64)
        < *total.read();

    rsx! {
        Card {
            title: "Activity Log",
            tight: true,
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "Recent server events — sign-ins, playback, task failures, and account changes."
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if entries.read().is_empty() {
                EmptyState { message: "No activity recorded yet." }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for entry in entries.read().clone() {
                            {
                                let id = entry.id.unwrap_or(0);
                                let name = entry.name.clone().unwrap_or_default();
                                let kind = entry.type_.clone().unwrap_or_default();
                                let overview = entry.overview.clone().unwrap_or_default();
                                let severity = entry.severity.clone().unwrap_or_else(|| "Information".to_string());
                                let color = severity_color(&severity);
                                let when = entry.date
                                    .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")))
                                    .unwrap_or_else(|| "—".to_string());
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--row-hover)] even:bg-[var(--row-stripe)] even:hover:bg-[var(--row-hover)]",
                                        key: "{id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem", "{name}" }
                                            if !overview.is_empty() {
                                                div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px;word-break:break-word", "{overview}" }
                                            }
                                            div { style: "font-size:.72rem;margin-top:2px",
                                                span { style: "color:{color};font-weight:500", "{severity}" }
                                                span { style: "color:var(--text-muted)", " · {kind} · {when}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if has_more {
                    div { style: "padding:10px 12px;text-align:center",
                        button {
                            class: "btn btn-ghost",
                            disabled: *loading_more.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    loading_more.set(true);
                                    let c = client.clone();
                                    let start = entries.read().len() as i64;
                                    spawn(async move {
                                        match c.execute(GetActivityLog {
                                            start_index: Some(start),
                                            limit: Some(PAGE_SIZE),
                                            has_user_id: None,
                                        }).await {
                                            Ok(result) => {
                                                total.set(result.total_record_count);
                                                entries.write().extend(result.items);
                                            }
                                            Err(e) => error.set(Some(format!("Failed to load more: {e}"))),
                                        }
                                        loading_more.set(false);
                                    });
                                }
                            },
                            if *loading_more.read() { "Loading…" } else { "Load more" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_color_maps_levels() {
        assert_eq!(severity_color("Error"), "var(--error)");
        assert_eq!(severity_color("Critical"), "var(--error)");
        assert_eq!(severity_color("Warning"), "var(--warning, #f59e0b)");
        assert_eq!(severity_color("Information"), "var(--text-muted)");
        assert_eq!(severity_color("anything-else"), "var(--text-muted)");
    }
}
