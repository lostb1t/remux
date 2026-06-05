use crate::{
    components::{Card, EmptyState, FormGroup, LoadingText},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{AuthenticationInfo, CreateApiKey, DeleteApiKey, GetApiKeys};

#[component]
pub fn ApiKeysPage(app_state: AppState) -> Element {
    let mut keys: Signal<Vec<AuthenticationInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Create-key dialog state
    let mut show_create = use_signal(|| false);
    let mut app_name_input = use_signal(String::new);
    let mut creating = use_signal(|| false);

    // Reveal dialog — shows the new key once after creation
    let mut revealed_key = use_signal(|| Option::<AuthenticationInfo>::None);

    // Confirm-delete state
    let mut key_to_delete: Signal<Option<String>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetApiKeys)
                .await
            {
                Ok(result) => {
                    keys.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load API keys: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card {
            title: "API Keys",
            tight: true,
            action: rsx! {
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        app_name_input.set(String::new());
                        show_create.set(true);
                    },
                    "+ New API Key"
                }
            },
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "API keys allow external applications to communicate with the server without a user login."
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if keys.read().is_empty() {
                EmptyState { message: "No API keys — create one to get started." }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for key in keys.read().clone() {
                                {
                                    let token = key.access_token.clone().unwrap_or_default();
                                    let app = key.app_name.clone().unwrap_or_default();
                                    let created = key.date_created
                                        .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")))
                                        .unwrap_or_else(|| "—".to_string());
                                     let token_del = token.clone();
                                    rsx! {
                                        div {
                                            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                            key: "{token}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { style: "font-weight:500;font-size:.85rem", "{app}" }
                                                div { style: "font-size:.72rem;color:var(--text-muted);font-family:monospace;margin-top:2px;word-break:break-all", "{token}" }
                                                div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px", "Created: {created}" }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                    onclick: move |_| key_to_delete.set(Some(token_del.clone())),
                                                    "Revoke"
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

        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "New API Key" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.8rem;color:var(--text-muted);margin-bottom:12px",
                            "Enter a name to identify the application using this key."
                        }
                        FormGroup { label: "App name",
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "e.g. My Media App",
                                value: "{app_name_input}",
                                oninput: move |e| app_name_input.set(e.value()),
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *creating.read() || app_name_input.read().trim().is_empty(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let name = app_name_input.read().trim().to_string();
                                    if name.is_empty() { return; }
                                    creating.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(CreateApiKey { app: name }).await {
                                            Ok(new_key) => {
                                                show_create.set(false);
                                                revealed_key.set(Some(new_key));
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to create key: {e}")));
                                                show_create.set(false);
                                            }
                                        }
                                        creating.set(false);
                                    });
                                }
                            },
                            if *creating.read() { "Creating…" } else { "Create" }
                        }
                    }
                }
            }
        }

        if let Some(new_key) = revealed_key.read().clone() {
            {
                let token = new_key.access_token.clone().unwrap_or_default();
                let app = new_key.app_name.clone().unwrap_or_default();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "API Key Created" }
                            }
                            div { class: "modal-body",
                                p { style: "font-size:.8rem;color:var(--text-muted);margin-bottom:12px",
                                    "Your new API key for "{app}" has been created. Copy it now — it will not be shown again."
                                }
                                FormGroup { label: "API Key",
                                    div { style: "display:flex;gap:6px;align-items:center",
                                        input {
                                            class: "form-input",
                                            r#type: "text",
                                            readonly: true,
                                            value: "{token}",
                                            style: "font-family:monospace;font-size:.8rem",
                                        }
                                        button {
                                            class: "btn btn-ghost",
                                            style: "height:36px;white-space:nowrap;flex-shrink:0",
                                            onclick: {
                                                let t = token.clone();
                                                move |_| {
                                                    if let Some(win) = web_sys::window() {
                                                        let _ = win.navigator().clipboard().write_text(&t);
                                                    }
                                                }
                                            },
                                            "Copy"
                                        }
                                    }
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-primary",
                                    onclick: move |_| revealed_key.set(None),
                                    "Done"
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(token) = key_to_delete.read().clone() {
            {
                let client = app_state.client.clone();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "Revoke API Key" }
                            }
                            div { class: "modal-body",
                                p { style: "font-size:.85rem",
                                    "Are you sure you want to revoke this key? Any application using it will lose access immediately."
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| key_to_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-ghost",
                                    style: "color:var(--error);border-color:var(--error)",
                                    disabled: *deleting.read(),
                                    onclick: {
                                        let t = token.clone();
                                        let c = client.clone();
                                        move |_| {
                                            deleting.set(true);
                                            let tok = t.clone();
                                            let cc = c.clone();
                                            spawn(async move {
                                                let _ = cc.execute(DeleteApiKey { key: tok }).await;
                                                key_to_delete.set(None);
                                                deleting.set(false);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            });
                                        }
                                    },
                                    if *deleting.read() { "Revoking…" } else { "Revoke" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
