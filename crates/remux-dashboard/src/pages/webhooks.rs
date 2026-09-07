use crate::{
    components::{
        Card, ErrorAlert, FormGroup, LoadingText, Modal, SuccessAlert, Switch,
    },
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CreateWebhook, DeleteWebhook, GetUsers, GetWebhooks, HttpWebhookConfig,
    UpdateWebhook, UserDto, WebhookConfig, WebhookDestination, WebhookEvent,
};
use std::collections::HashMap;
use uuid::Uuid;

const WEBHOOK_EVENTS: &[WebhookEvent] = &[
    WebhookEvent::PlaybackStart,
    WebhookEvent::PlaybackProgress,
    WebhookEvent::PlaybackStop,
    WebhookEvent::UserDataSaved,
    WebhookEvent::UserUpdated,
    WebhookEvent::UserDeleted,
];

#[component]
pub fn WebhooksPage(app_state: AppState) -> Element {
    let mut hooks = use_signal(Vec::<WebhookConfig>::new);
    let mut show_editor = use_signal(|| false);
    let mut editing_id = use_signal(|| Option::<Uuid>::None);
    let mut editing_config = use_signal(|| Option::<WebhookConfig>::None);
    let mut name = use_signal(String::new);
    let mut url = use_signal(String::new);
    let mut template = use_signal(String::new);
    let mut headers = use_signal(Vec::<(String, String)>::new);
    let mut enabled = use_signal(|| true);
    let mut selected_events = use_signal(|| {
        WEBHOOK_EVENTS
            .iter()
            .take(4)
            .copied()
            .collect::<Vec<_>>()
    });
    let mut selected_user_ids = use_signal(Vec::<Uuid>::new);
    let mut users = use_signal(Vec::<UserDto>::new);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let load_client = app_state.clone();
    use_effect(move || {
        let client = load_client.clone();
        spawn(async move {
            match client
                .execute(GetWebhooks)
                .await
            {
                Ok(items) => hooks.set(items),
                Err(e) => error.set(Some(e.user_message())),
            }
            loading.set(false);
        });
    });

    let users_client = app_state.clone();
    use_effect(move || {
        let client = users_client.clone();
        spawn(async move {
            if let Ok(list) = client
                .execute(GetUsers)
                .await
            {
                users.set(list);
            }
        });
    });

    let mut open_new = move || {
        editing_id.set(None);
        editing_config.set(None);
        name.set(String::new());
        url.set(String::new());
        template.set(String::new());
        headers.set(Vec::new());
        enabled.set(true);
        selected_events.set(
            WEBHOOK_EVENTS
                .iter()
                .take(4)
                .copied()
                .collect(),
        );
        selected_user_ids.set(Vec::new());
        error.set(None);
        show_editor.set(true);
    };
    let mut open_edit = move |hook: WebhookConfig| {
        editing_id.set(Some(hook.id));
        editing_config.set(Some(hook.clone()));
        name.set(hook.name);
        template.set(hook.template);
        let WebhookDestination::Http(http) = hook.destination;
        url.set(http.url);
        let mut header_rows: Vec<_> = http
            .headers
            .into_iter()
            .collect();
        header_rows.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
        });
        headers.set(header_rows);
        enabled.set(hook.enabled);
        selected_events.set(hook.events);
        selected_user_ids.set(hook.user_ids);
        error.set(None);
        show_editor.set(true);
    };

    let save_client = app_state.clone();
    let save = move |e: Event<FormData>| {
        e.prevent_default();
        if selected_events
            .peek()
            .is_empty()
        {
            error.set(Some("Select at least one event".to_string()));
            return;
        }
        saving.set(true);
        error.set(None);
        saved.set(false);
        let existing = *editing_id.peek();
        let mut config = editing_config
            .peek()
            .clone()
            .unwrap_or_default();
        config.id = existing.unwrap_or_else(Uuid::new_v4);
        config.name = name
            .peek()
            .trim()
            .to_string();
        config.enabled = *enabled.peek();
        config.destination = WebhookDestination::Http(HttpWebhookConfig {
            url: url
                .peek()
                .trim()
                .to_string(),
            headers: headers
                .peek()
                .iter()
                .filter_map(|(key, value)| {
                    let key = key.trim();
                    (!key.is_empty()).then(|| (key.to_string(), value.clone()))
                })
                .collect::<HashMap<_, _>>(),
        });
        config.events = selected_events
            .peek()
            .clone();
        config.user_ids = selected_user_ids
            .peek()
            .clone();
        config.template = template
            .peek()
            .clone();
        let client = save_client.clone();
        spawn(async move {
            let result = if existing.is_some() {
                client
                    .execute(UpdateWebhook { config })
                    .await
            } else {
                client
                    .execute(CreateWebhook { config })
                    .await
            };
            match result {
                Ok(item) => {
                    let mut list = hooks
                        .peek()
                        .clone();
                    if let Some(pos) = list
                        .iter()
                        .position(|entry| entry.id == item.id)
                    {
                        list[pos] = item;
                    } else {
                        list.push(item);
                    }
                    hooks.set(list);
                    show_editor.set(false);
                    saved.set(true);
                }
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };
    let action_client = app_state.clone();

    rsx! {
        Card { title: "Webhooks",
            if *loading.read() { LoadingText {} } else {
                if let Some(message) = error.read().as_ref() { ErrorAlert { message: message.clone() } }
                if *saved.read() { SuccessAlert { message: "Webhook saved" } }
                div { class: "webhooks-toolbar", button { class: "btn btn-primary", onclick: move |_| open_new(), "Add webhook" } }
                if hooks.read().is_empty() { div { class: "empty-state", "No webhooks configured" } } else {
                    div { class: "webhooks-table-wrap",
                        table { class: "webhooks-table",
                            thead { tr { th { "Name" } th { "URL" } th { "Events" } th { "Status" } th { "Actions" } } }
                            tbody { for hook in hooks.read().iter() { {
                                let WebhookDestination::Http(destination) = &hook.destination;
                                let endpoint = destination.url.clone();
                                rsx! { tr {
                                    td { "{hook.name}" }
                                    td { class: "webhooks-url", title: "{endpoint}", "{endpoint}" }
                                    td { "{hook.events.len()} selected" }
                                    td { if hook.enabled { span { class: "webhooks-status webhooks-status-on", "Enabled" } } else { span { class: "webhooks-status", "Disabled" } } }
                                    td { class: "webhooks-actions",
                                        button { class: "btn btn-ghost", onclick: { let hook = hook.clone(); move |_| open_edit(hook.clone()) }, "Edit" }
                                        button { class: "btn btn-danger", onclick: { let id = hook.id; let client = action_client.clone(); move |_| { let client = client.clone(); spawn(async move { if let Err(e) = client.execute(DeleteWebhook { id }).await { error.set(Some(e.user_message())); } else { hooks.write().retain(|entry| entry.id != id); } }); } }, "Delete" }
                                    }
                                } }
                            } } }
                        }
                    }
                }
            }
        }
        if *show_editor.read() { Modal { on_close: move |_| show_editor.set(false),
            div { class: "modal-header", span { class: "modal-title", if editing_id.read().is_some() { "Edit Webhook" } else { "New Webhook" } } }
            form { onsubmit: save,
                div { class: "modal-body",
                    FormGroup { label: "Name", input { class: "form-input", value: "{name}", oninput: move |e| name.set(e.value()) } }
                    FormGroup { label: "URL", input { class: "form-input", r#type: "url", value: "{url}", oninput: move |e| url.set(e.value()) } }
                    div { class: "form-group", div { class: "toggle-row", div { class: "toggle-row-text", span { class: "toggle-label", "Enabled" } }, Switch { checked: *enabled.read(), on_change: move |value| enabled.set(value) } } }
                    div { class: "form-group", label { class: "form-label", "Events" }, div { class: "webhook-event-switches", for event_name in WEBHOOK_EVENTS.iter() { {
                        let event_name = *event_name;
                        let checked = selected_events.read().contains(&event_name);
                        rsx! { div { class: "toggle-row webhook-event-switch", div { class: "toggle-row-text", span { class: "toggle-label", "{event_name}" } }, Switch { checked, on_change: move |value| { let mut events = selected_events.write(); if value { if !events.contains(&event_name) { events.push(event_name); } } else { events.retain(|selected| selected != &event_name); } } } } }
                    } } } }
                    div { class: "form-group",
                        label { class: "form-label", "Users" }
                        div { class: "webhook-hint", "Leave all off to notify for every user" }
                        div { class: "webhook-event-switches", for user in users.read().iter() { {
                            let user_id = user.id;
                            let user_name = user.name.clone();
                            let checked = selected_user_ids.read().contains(&user_id);
                            rsx! { div { class: "toggle-row webhook-event-switch", div { class: "toggle-row-text", span { class: "toggle-label", "{user_name}" } }, Switch { checked, on_change: move |value| { let mut ids = selected_user_ids.write(); if value { if !ids.contains(&user_id) { ids.push(user_id); } } else { ids.retain(|selected| selected != &user_id); } } } } }
                        } } }
                    }
                    div { class: "form-group",
                        label { class: "form-label", "HTTP headers" }
                        div { class: "webhook-header-list",
                            for (index, (header_name, header_value)) in headers.read().iter().cloned().enumerate() {
                                div { class: "webhook-header-row",
                                    input {
                                        class: "form-input",
                                        placeholder: "Header name",
                                        value: "{header_name}",
                                        oninput: move |event| {
                                            if let Some(header) = headers.write().get_mut(index) {
                                                header.0 = event.value();
                                            }
                                        }
                                    }
                                    input {
                                        class: "form-input",
                                        placeholder: "Value",
                                        value: "{header_value}",
                                        oninput: move |event| {
                                            if let Some(header) = headers.write().get_mut(index) {
                                                header.1 = event.value();
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn btn-danger webhook-header-delete",
                                        r#type: "button",
                                        onclick: move |_| {
                                            headers.write().remove(index);
                                        },
                                        "Delete"
                                    }
                                }
                            }
                        }
                        button {
                            class: "btn btn-ghost webhook-add-header",
                            r#type: "button",
                            onclick: move |_| headers.write().push((String::new(), String::new())),
                            "Add header"
                        }
                    }
                    FormGroup { label: "Handlebars template", textarea { class: "form-input", style: "min-height:180px;font-family:var(--font-mono);resize:vertical", value: "{template}", oninput: move |e| template.set(e.value()) } }
                }
                div { class: "modal-footer",
                    button { class: "btn btn-ghost", r#type: "button", onclick: move |_| show_editor.set(false), "Cancel" }
                    button { class: "btn btn-primary", disabled: *saving.read(), r#type: "submit", if *saving.read() { "Saving…" } else { "Save" } }
                }
            }
        } }
    }
}
