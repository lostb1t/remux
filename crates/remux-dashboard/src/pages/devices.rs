use crate::{
    components::{Card, EmptyState, FormGroup, LoadingText, Modal},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{DeleteDevice, DeviceInfo, GetDevices, SetDeviceOptions};

/// Devices manager: lists the client devices that have logged in, and lets an
/// admin give a device a friendly custom name or revoke it. Backed by the
/// Jellyfin-compatible `/Devices`, `/Devices/Options` endpoints.
#[component]
pub fn DevicesPage(app_state: AppState) -> Element {
    let mut devices: Signal<Vec<DeviceInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Rename dialog state.
    let mut rename_target: Signal<Option<DeviceInfo>> = use_signal(|| None);
    let mut rename_input = use_signal(String::new);
    let mut saving = use_signal(|| false);

    // Confirm-delete state.
    let mut device_to_delete: Signal<Option<String>> = use_signal(|| None);
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
                .execute(GetDevices)
                .await
            {
                Ok(result) => {
                    devices.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load devices: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card {
            title: "Devices",
            tight: true,
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "Every client that signs in registers a device. Rename a device to recognise it, or revoke it to sign it out."
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if devices.read().is_empty() {
                EmptyState { message: "No devices have connected yet." }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for device in devices.read().clone() {
                            {
                                let id = device.id.clone().unwrap_or_default();
                                let reported = device.name.clone().unwrap_or_default();
                                let custom = device.custom_name.clone().unwrap_or_default();
                                let display = if custom.is_empty() { reported.clone() } else { custom.clone() };
                                let app = format!(
                                    "{} {}",
                                    device.app_name.clone().unwrap_or_default(),
                                    device.app_version.clone().unwrap_or_default(),
                                );
                                let last_user = device.last_user_name.clone().unwrap_or_default();
                                let last_active = device.date_last_activity
                                    .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")))
                                    .unwrap_or_else(|| "—".to_string());
                                let dev_for_rename = device.clone();
                                let id_for_delete = id.clone();
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[var(--row-hover)] even:bg-[var(--row-stripe)] even:hover:bg-[var(--row-hover)]",
                                        key: "{id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem", "{display}" }
                                            if !custom.is_empty() {
                                                div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px", "Reported as {reported}" }
                                            }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px", "{app}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px",
                                                "Last user: {last_user} · Last active: {last_active}"
                                            }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    rename_input.set(dev_for_rename.custom_name.clone().unwrap_or_default());
                                                    rename_target.set(Some(dev_for_rename.clone()));
                                                },
                                                "Rename"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| device_to_delete.set(Some(id_for_delete.clone())),
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

        if let Some(device) = rename_target.read().clone() {
            {
                let id = device.id.clone().unwrap_or_default();
                let reported = device.name.clone().unwrap_or_default();
                let client = app_state.client.clone();
                rsx! {
                    Modal { on_close: move |_| rename_target.set(None),
                        div { class: "modal-header",
                            span { class: "modal-title", "Rename Device" }
                        }
                        div { class: "modal-body",
                            p { style: "font-size:.8rem;color:var(--text-muted);margin-bottom:12px",
                                "Set a custom name for “"{reported}"”. Leave blank to use the name the client reports."
                            }
                            FormGroup { label: "Custom name",
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "e.g. Living Room TV",
                                    value: "{rename_input}",
                                    oninput: move |e| rename_input.set(e.value()),
                                }
                            }
                        }
                        div { class: "modal-footer",
                            button {
                                class: "btn btn-ghost",
                                onclick: move |_| rename_target.set(None),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                onclick: {
                                    let id = id.clone();
                                    let c = client.clone();
                                    move |_| {
                                        saving.set(true);
                                        let name = rename_input.read().trim().to_string();
                                        let id = id.clone();
                                        let c = c.clone();
                                        spawn(async move {
                                            match c.execute(SetDeviceOptions { id, custom_name: name }).await {
                                                Ok(_) => { rename_target.set(None); }
                                                Err(e) => error.set(Some(format!("Failed to rename device: {e}"))),
                                            }
                                            saving.set(false);
                                            let v = *refresh.peek() + 1;
                                            refresh.set(v);
                                        });
                                    }
                                },
                                if *saving.read() { "Saving…" } else { "Save" }
                            }
                        }
                    }
                }
            }
        }

        if let Some(id) = device_to_delete.read().clone() {
            {
                let client = app_state.client.clone();
                rsx! {
                    Modal { on_close: move |_| device_to_delete.set(None),
                        div { class: "modal-header",
                            span { class: "modal-title", "Revoke Device" }
                        }
                        div { class: "modal-body",
                            p { style: "font-size:.85rem",
                                "Revoke this device? It will be signed out and must log in again."
                            }
                        }
                        div { class: "modal-footer",
                            button {
                                class: "btn btn-ghost",
                                onclick: move |_| device_to_delete.set(None),
                                "Cancel"
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "color:var(--error);border-color:var(--error)",
                                disabled: *deleting.read(),
                                onclick: {
                                    let id = id.clone();
                                    let c = client.clone();
                                    move |_| {
                                        deleting.set(true);
                                        let id = id.clone();
                                        let c = c.clone();
                                        spawn(async move {
                                            let _ = c.execute(DeleteDevice { id }).await;
                                            device_to_delete.set(None);
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
