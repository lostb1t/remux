use crate::{
    components::{Card, EmptyState, LoadingText, Modal},
    state::{fmt_time, get_origin, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{GetLogFiles, LogFile};

/// Human-readable byte size (e.g. `1.5 KB`). Pure so it can be unit-tested.
fn humanize_size(bytes: i64) -> String {
    if bytes < 0 {
        return "—".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Build the authenticated download URL for a log file. The server accepts the
/// access token as an `api_key` query parameter, so a plain anchor/`fetch` works
/// without setting request headers. Pure (origin passed in) so it is unit-testable.
fn log_url(origin: &str, token: &str, name: &str) -> String {
    format!(
        "{}/system/logs/log?name={}&api_key={}",
        origin.trim_end_matches('/'),
        urlencoding::encode(name),
        urlencoding::encode(token),
    )
}

/// Server logs viewer: lists the log files in the server's `log_dir` and lets an
/// admin view a file inline or download it. Backed by `/System/Logs`.
#[component]
pub fn LogsPage(app_state: AppState) -> Element {
    let mut files: Signal<Vec<LogFile>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    // Inline viewer state.
    let mut viewing: Signal<Option<String>> = use_signal(|| None); // file name
    let mut view_body = use_signal(String::new);
    let mut view_loading = use_signal(|| false);

    let token = app_state
        .server
        .access_token
        .clone();
    // Prefer the live browser origin; fall back to the stored server address.
    let origin = {
        let o = get_origin();
        if o.is_empty() {
            app_state
                .server
                .manual_address
                .clone()
        } else {
            o
        }
    };

    let app_state_effect = app_state.clone();
    use_effect(move || {
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetLogFiles)
                .await
            {
                Ok(list) => {
                    files.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load logs: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card {
            title: "Logs",
            tight: true,
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "Server log files. View a file inline or download it for sharing."
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if files.read().is_empty() {
                EmptyState { message: "No log files yet." }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for file in files.read().clone() {
                            {
                                let name = file.name.clone().unwrap_or_default();
                                let size = humanize_size(file.size.unwrap_or(0));
                                let modified = file.date_modified
                                    .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")))
                                    .unwrap_or_else(|| "—".to_string());
                                let download = log_url(&origin, &token, &name);
                                let name_for_view = name.clone();
                                let url_for_view = download.clone();
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--row-hover)] even:bg-[var(--row-stripe)] even:hover:bg-[var(--row-hover)]",
                                        key: "{name}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem;font-family:monospace", "{name}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px", "{size} · Modified {modified}" }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    let url = url_for_view.clone();
                                                    viewing.set(Some(name_for_view.clone()));
                                                    view_body.set(String::new());
                                                    view_loading.set(true);
                                                    spawn(async move {
                                                        let text = match gloo_net::http::Request::get(&url).send().await {
                                                            Ok(resp) => resp.text().await.unwrap_or_else(|e| format!("Failed to read log: {e}")),
                                                            Err(e) => format!("Failed to fetch log: {e}"),
                                                        };
                                                        view_body.set(text);
                                                        view_loading.set(false);
                                                    });
                                                },
                                                "View"
                                            }
                                            a {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;display:inline-flex;align-items:center",
                                                href: "{download}",
                                                download: "{name}",
                                                "Download"
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

        if let Some(name) = viewing.read().clone() {
            Modal { size: crate::components::ModalSize::Wide, on_close: move |_| viewing.set(None),
                div { class: "modal-header",
                    span { class: "modal-title", style: "font-family:monospace", "{name}" }
                }
                div { class: "modal-body",
                    if *view_loading.read() {
                        LoadingText {}
                    } else {
                        pre {
                            style: "max-height:60vh;overflow:auto;font-size:.72rem;font-family:monospace;white-space:pre-wrap;word-break:break-word;background:var(--bg);padding:10px;border-radius:6px;margin:0",
                            "{view_body}"
                        }
                    }
                }
                div { class: "modal-footer",
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| viewing.set(None),
                        "Close"
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
    fn humanize_size_scales_units() {
        assert_eq!(humanize_size(0), "0 B");
        assert_eq!(humanize_size(512), "512 B");
        assert_eq!(humanize_size(1024), "1.0 KB");
        assert_eq!(humanize_size(1536), "1.5 KB");
        assert_eq!(humanize_size(1024 * 1024), "1.0 MB");
        assert_eq!(humanize_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(humanize_size(-1), "—");
    }

    #[test]
    fn log_url_encodes_name_and_token() {
        let url = log_url("https://example.test", "tok en", "remux.log");
        assert!(url.ends_with("/system/logs/log?name=remux.log&api_key=tok%20en"));
        // A name that needs encoding stays safe.
        let url2 = log_url("https://example.test/", "t", "a b.log");
        assert!(url2.contains("name=a%20b.log"));
    }
}
