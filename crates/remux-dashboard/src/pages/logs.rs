use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{GetLogs, LogEntry};

#[component]
pub fn LogsPage(app_state: AppState) -> Element {
    let mut entries = use_signal(Vec::<LogEntry>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut search = use_signal(String::new);
    let mut level = use_signal(|| "all".to_string());
    let mut refresh = use_signal(|| 0_u32);
    let mut request_id = use_signal(|| 0_u64);

    let client = app_state.clone();
    use_effect(move || {
        let _refresh = *refresh.read();
        let query = search
            .read()
            .clone();
        let level_filter = level
            .read()
            .clone();
        let request = *request_id.peek() + 1;
        request_id.set(request);
        let request_id_for_response = request_id;
        let client = client.clone();
        loading.set(true);
        spawn(async move {
            let result = client
                .execute(GetLogs {
                    limit: None,
                    level: Some(level_filter),
                    search: if query.is_empty() { None } else { Some(query) },
                })
                .await;
            if *request_id_for_response.peek() != request {
                return;
            }
            match result {
                Ok(result) => {
                    entries.set(result.entries);
                    error.set(None);
                }
                Err(err) => {
                    error.set(Some(format!("Failed to load server logs: {err}")))
                }
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Server Logs".to_string(), tight: true,
            div { class: "logs-toolbar",
                input {
                    r#type: "search",
                    class: "field-input logs-search",
                    aria_label: "Search log messages",
                    placeholder: "Search log messages…",
                    value: "{search.read()}",
                    oninput: move |evt| search.set(evt.value()),
                }
                div { class: "logs-level-field",
                    label { r#for: "log-level", class: "field-label", "Level" }
                    select {
                        id: "log-level",
                        class: "select-input logs-level-select",
                        value: "{level.read()}",
                        onchange: move |evt| level.set(evt.value()),
                        option { value: "all", "All levels" }
                        option { value: "trace", "Trace" }
                        option { value: "debug", "Debug" }
                        option { value: "info", "Info" }
                        option { value: "warn", "Warn" }
                        option { value: "error", "Error" }
                    }
                }
                button {
                    class: "btn btn-ghost logs-refresh",
                    disabled: *loading.read(),
                    onclick: move |_| { let next = *refresh.peek() + 1; refresh.set(next); },
                    "Refresh"
                }
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(message) = error.read().as_ref() {
                ErrorAlert { message: message.clone() }
            } else if entries.read().is_empty() {
                EmptyState { message: "No log entries match these filters" }
            } else {
                div { class: "data-table-container", style: "max-height:70vh;overflow:auto",
                    for (index, entry) in entries.read().iter().enumerate() {
                        div {
                            key: "{index}-{entry.timestamp}-{entry.level}",
                            style: "display:grid;grid-template-columns:90px 64px minmax(0,1fr);gap:10px;padding:8px 10px;border-bottom:1px solid var(--border);font-family:monospace;font-size:.78rem;white-space:pre-wrap;overflow-wrap:anywhere",
                            span { style: "color:var(--text-dim)", "{entry.timestamp}" }
                            span { style: "font-weight:600", "{entry.level}" }
                            span { "{entry.message}" }
                        }
                    }
                }
            }
        }
    }
}
