use crate::{
    components::{
        Card, ErrorAlert, FormGroup, LoadingText, Modal, SuccessAlert, Switch,
    },
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CreateWebhook, DeleteWebhook, GetWebhooks, UpdateWebhook, WebhookConfig,
    WebhookEvent,
};
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
    let mut name = use_signal(String::new);
    let mut url = use_signal(String::new);
    let mut template = use_signal(String::new);
    let mut enabled = use_signal(|| true);
    let mut selected_events = use_signal(|| {
        WEBHOOK_EVENTS
            .iter()
            .take(4)
            .copied()
            .collect::<Vec<_>>()
    });
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

    let mut open_new = move || {
        editing_id.set(None);
        name.set(String::new());
        url.set(String::new());
        template.set(String::new());
        enabled.set(true);
        selected_events.set(
            WEBHOOK_EVENTS
                .iter()
                .take(4)
                .copied()
                .collect(),
        );
        error.set(None);
        show_editor.set(true);
    };
    let mut open_edit = move |hook: WebhookConfig| {
        editing_id.set(Some(hook.id));
        name.set(hook.name);
        url.set(hook.url);
        template.set(hook.template);
        enabled.set(hook.enabled);
        selected_events.set(hook.events);
        error.set(None);
        show_editor.set(true);
    };

    let save_client = app_state.clone();
    let save = move |e: Event<FormData>| {
        e.prevent_default();
        saving.set(true);
        error.set(None);
        saved.set(false);
        let existing = *editing_id.peek();
        let config = WebhookConfig {
            id: existing.unwrap_or_else(Uuid::new_v4),
            name: name
                .peek()
                .trim()
                .to_string(),
            enabled: *enabled.peek(),
            url: url
                .peek()
                .trim()
                .to_string(),
            events: selected_events
                .peek()
                .clone(),
            template: template
                .peek()
                .clone(),
            ..Default::default()
        };
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
                            tbody { for hook in hooks.read().iter() { tr {
                                td { "{hook.name}" }
                                td { class: "webhooks-url", title: "{hook.url}", "{hook.url}" }
                                td { "{hook.events.len()} selected" }
                                td { if hook.enabled { span { class: "webhooks-status webhooks-status-on", "Enabled" } } else { span { class: "webhooks-status", "Disabled" } } }
                                td { class: "webhooks-actions",
                                    button { class: "btn btn-ghost", onclick: { let hook = hook.clone(); move |_| open_edit(hook.clone()) }, "Edit" }
                                    button { class: "btn btn-danger", onclick: { let id = hook.id; let client = action_client.clone(); move |_| { let client = client.clone(); spawn(async move { if let Err(e) = client.execute(DeleteWebhook { id }).await { error.set(Some(e.user_message())); } else { hooks.write().retain(|entry| entry.id != id); } }); } }, "Delete" }
                                }
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
