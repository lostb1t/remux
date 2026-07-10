use crate::{
    components::{Card, EmptyState, LoadingText},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{GetSessions, SessionInfoDto};

#[component]
pub fn SessionsCard(app_state: AppState) -> Element {
    let mut sessions: Signal<Vec<SessionInfoDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state
            .client
            .clone();
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
                Err(e) => error.set(Some(format!("Failed to fetch sessions: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Active Devices", tight: true,
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if sessions.read().is_empty() {
                EmptyState { message: "No active devices in the last 16 minutes" }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for session in sessions.read().iter() {
                            div { class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--row-hover)] even:bg-[var(--row-stripe)] even:hover:bg-[var(--row-hover)]",
                                div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                    div { class: "session-name",
                                        "{session.device_name.as_deref().unwrap_or(\"Unknown device\")}"
                                    }
                                    if let Some(item) = &session.now_playing_item {
                                        div { class: "session-playing",
                                            "▶ {item.name.as_deref().unwrap_or(\"Unknown\")}"
                                        }
                                    }
                                }
                                div { class: "shrink-0 px-3 py-[10px]",
                                    div { class: "session-user",
                                        "{session.user_name.as_deref().unwrap_or(\"Unknown\")}"
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
