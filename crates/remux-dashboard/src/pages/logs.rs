use crate::{
    components::{Card, EmptyState, ErrorAlert, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use remux_sdks::remux::{GetLogs, LogEntry};
use wasm_bindgen::JsCast;

#[component]
pub fn LogsPage(app_state: AppState) -> Element {
    let mut entries = use_signal(Vec::<LogEntry>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut search = use_signal(String::new);
    let debounced_search = use_signal(String::new);
    let mut debounce_generation = use_signal(|| 0_u64);
    let mut level = use_signal(|| "all".to_string());
    let mut refresh = use_signal(|| 0_u32);
    let mut request_id = use_signal(|| 0_u64);

    use_effect(move || {
        let _entry_count = entries
            .read()
            .len();
        if let Some(list) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("logs-list"))
            .and_then(|element| {
                element
                    .dyn_into::<web_sys::HtmlElement>()
                    .ok()
            })
        {
            // The API returns newest-first, so the top of the scroll area is the newest entry.
            list.set_scroll_top(0);
        }
    });

    let client = app_state.clone();
    use_effect(move || {
        let _refresh = *refresh.read();
        let query = debounced_search
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
                    oninput: move |evt| {
                        let value = evt.value();
                        search.set(value.clone());
                        let generation = *debounce_generation.peek() + 1;
                        debounce_generation.set(generation);
                        let generation_signal = debounce_generation;
                        let mut debounced_signal = debounced_search;
                        spawn(async move {
                            TimeoutFuture::new(300).await;
                            if *generation_signal.peek() == generation {
                                debounced_signal.set(value);
                            }
                        });
                    },
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
                div { class: "logs-disclaimer", role: "note",
                    "Server logs are admin-only. Common credential fields are redacted, but log messages may still contain sensitive data."
                }
                div { id: "logs-list", class: "logs-list", role: "table", aria_label: "Server log entries",
                    for (index, entry) in entries.read().iter().enumerate() {
                        div {
                            key: "{index}-{entry.timestamp}-{entry.level}",
                            class: "logs-row",
                            role: "row",
                            span { class: "logs-timestamp", role: "cell", "{entry.timestamp}" }
                            span { class: "logs-level", role: "cell", "{entry.level}" }
                            span { class: "logs-message", role: "cell", "{entry.message}" }
                        }
                    }
                }
            }
        }
    }
}
