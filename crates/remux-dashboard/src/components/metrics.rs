use crate::{
    components::{Card, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{GetMetricsStatus, MetricsStatus};

fn days_ago_label(days: i64) -> String {
    match days {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

#[component]
pub fn MetricsCard(app_state: AppState) -> Element {
    let mut status: Signal<Option<MetricsStatus>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetMetricsStatus)
                .await
            {
                Ok(s) => {
                    status.set(Some(s));
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to fetch metrics status: {e}")))
                }
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Popularity Metrics",
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if let Some(s) = status.read().as_ref() {
                if s.item_count == 0 {
                    span { class: "loading-text",
                        "No popularity data — run Refresh Popularity to start tracking."
                    }
                } else {
                    div { class: "kv-row",
                        span { class: "kv-label", "Last updated" }
                        span { class: "kv-value",
                            {
                                s.last_updated_days_ago
                                    .map(days_ago_label)
                                    .unwrap_or_else(|| "—".to_string())
                            }
                        }
                    }
                    div { class: "kv-row",
                        span { class: "kv-label", "Daily coverage" }
                        span {
                            class: "kv-value",
                            style: if s.daily_days < s.daily_window { "color:var(--warning, #f5a623)" } else { "" },
                            "{s.daily_days} / {s.daily_window} days"
                        }
                    }
                    div { class: "kv-row",
                        span { class: "kv-label", "Items tracked" }
                        span { class: "kv-value", "{s.item_count}" }
                    }
                }
            }
        }
    }
}
