use crate::{
    components::{Card, ConfirmDialog, EmptyState, ErrorAlert, LoadingText},
    state::{clear_credentials, fmt_datetime, fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::{
    ClientError,
    remux::{
        ActivityLogEntry, DeleteDevice, DeleteUserDevices, DeviceInfo, GetActivityLog,
        GetDevices, QueryResult,
    },
};
use std::collections::HashMap;

#[component]
pub fn SessionsCard(app_state: AppState) -> Element {
    let mut logged_in = use_context::<Signal<bool>>();
    let mut devices: Signal<Vec<DeviceInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let refresh = use_signal(|| 0_u32);
    let mut confirm_revoke: Signal<Option<String>> = use_signal(|| None);
    let mut confirm_revoke_user: Signal<Option<String>> = use_signal(|| None);
    let mut activity_items: Signal<Vec<ActivityLogEntry>> = use_signal(Vec::new);
    let mut activity_loading = use_signal(|| true);

    let app_state_devices = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_devices.client.clone();
        spawn(async move {
            match client.execute(GetDevices { user_id: None }).await {
                Ok(QueryResult { items, .. }) => {
                    devices.set(items);
                    error.set(None);
                }
                Err(ClientError::Unauthorized) => {
                    clear_credentials();
                    logged_in.set(false);
                }
                Err(e) => error.set(Some(format!("Failed to load devices: {e}"))),
            }
            loading.set(false);
        });
    });

    let app_state_activity = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        activity_loading.set(true);
        let client = app_state_activity.client.clone();
        spawn(async move {
            if let Ok(result) = client
                .execute(GetActivityLog {
                    start_index: Some(0),
                    limit: Some(50),
                })
                .await
            {
                activity_items.set(result.items);
            }
            activity_loading.set(false);
        });
    });

    // Group devices by user_id for "Revoke All" per-user action.
    let grouped: HashMap<String, Vec<DeviceInfo>> = {
        let mut map: HashMap<String, Vec<DeviceInfo>> = HashMap::new();
        for d in devices.read().iter() {
            let uid = d
                .remux
                .as_ref()
                .and_then(|r| r.user_id)
                .map(|u| u.to_string())
                .unwrap_or_default();
            map.entry(uid).or_default().push(d.clone());
        }
        map
    };

    let app_state_revoke = app_state.clone();
    let app_state_revoke_all = app_state.clone();

    rsx! {
        div { class: "flex flex-col gap-4",

            Card { title: "All Devices",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    ErrorAlert { message: err.clone() }
                } else if devices.read().is_empty() {
                    EmptyState { message: "No devices found" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for device in devices.read().clone() {
                                {
                                    let device_id = device.id.clone().unwrap_or_default();
                                    let is_self = device
                                        .remux
                                        .as_ref()
                                        .and_then(|r| r.is_current_session)
                                        .unwrap_or(false);
                                    let device_id_revoke = device_id.clone();
                                    let remote_ip = device.remux.as_ref().and_then(|r| r.remote_end_point.clone());
                                    rsx! {
                                        div {
                                            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                            key: "{device_id}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "session-name",
                                                    "{device.name.as_deref().unwrap_or(\"Unknown device\")}"
                                                    if is_self {
                                                        span { class: "user-badge user-badge-self", style: "margin-left:6px", "This session" }
                                                    }
                                                }
                                                div { class: "text-xs text-[var(--text-dim)] mt-0.5",
                                                    if let Some(ip) = &remote_ip {
                                                        span { "{ip}" }
                                                    }
                                                    if let Some(created) = device.date_created {
                                                        span { style: "margin-left:6px",
                                                            "First seen: {fmt_time(created)}"
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px]",
                                                if let Some(user) = &device.last_user_name {
                                                    div { class: "session-user", "{user}" }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px]",
                                                if let Some(app) = &device.app_name {
                                                    div { class: "session-client-badge",
                                                        "{app}"
                                                        if let Some(v) = &device.app_version {
                                                            " {v}"
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] text-right font-mono text-[var(--text-dim)] text-xs",
                                                if let Some(t) = device.date_last_activity {
                                                    "{fmt_time(t)}"
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px]",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:28px;font-size:.65rem;padding:0 8px;color:var(--error);border-color:var(--error)",
                                                    disabled: is_self,
                                                    onclick: move |_| confirm_revoke.set(Some(device_id_revoke.clone())),
                                                    "Revoke"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Per-user "Revoke All" buttons
                        div { class: "flex flex-wrap gap-2 px-3 py-3 border-t border-[var(--border)]",
                            for (uid, user_devices) in &grouped {
                                {
                                    let user_label = user_devices
                                        .first()
                                        .and_then(|d| d.last_user_name.clone())
                                        .unwrap_or_else(|| uid.clone());
                                    let uid_str = uid.clone();
                                    rsx! {
                                        button {
                                            class: "btn btn-ghost",
                                            style: "height:28px;font-size:.65rem;padding:0 8px;color:var(--error);border-color:var(--error)",
                                            onclick: move |_| confirm_revoke_user.set(Some(uid_str.clone())),
                                            "Revoke all for {user_label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Activity log
            Card { title: "Admin Activity Log",
                if *activity_loading.read() {
                    LoadingText {}
                } else if activity_items.read().is_empty() {
                    EmptyState { message: "No admin actions recorded yet" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for entry in activity_items.read().iter() {
                                div {
                                    class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                    key: "{entry.id.as_deref().unwrap_or(\"\")}",
                                    div { class: "shrink-0 px-3 py-[8px] font-mono text-xs text-[var(--text-dim)] w-40",
                                        if let Some(ts) = entry.date {
                                            "{fmt_datetime(ts)}"
                                        }
                                    }
                                    div { class: "shrink-0 px-3 py-[8px] text-xs text-[var(--text-dim)] w-32",
                                        "{entry.remux.as_ref().and_then(|r| r.user_name.as_deref()).unwrap_or(\"\")}"
                                    }
                                    div { class: "shrink-0 px-3 py-[8px] text-xs font-medium w-40",
                                        "{entry.name.as_deref().unwrap_or(\"\")}"
                                    }
                                    div { class: "flex-1 min-w-0 px-3 py-[8px] text-xs text-[var(--text-dim)]",
                                        if let Some(target) = entry.remux.as_ref().and_then(|r| r.target_user_name.as_deref()) {
                                            "user: {target}"
                                        }
                                        if let Some(dev) = entry.remux.as_ref().and_then(|r| r.device_name.as_deref()) {
                                            span { style: "margin-left:8px", "device: {dev}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Revoke single device confirmation
        if let Some(did) = confirm_revoke.read().clone() {
            ConfirmDialog {
                message: "Revoke this device? It will be signed out immediately.",
                on_confirm: {
                    let client = app_state_revoke.client.clone();
                    move |_| {
                        let did = did.clone();
                        let client = client.clone();
                        let mut cr = confirm_revoke.clone();
                        let mut ref_ = refresh.clone();
                        let mut err = error.clone();
                        spawn(async move {
                            match client.execute(DeleteDevice { id: did }).await {
                                Ok(_) => {
                                    cr.set(None);
                                    let v = *ref_.peek() + 1;
                                    ref_.set(v);
                                }
                                Err(e) => {
                                    cr.set(None);
                                    err.set(Some(format!("Failed to revoke session: {e}")));
                                }
                            }
                        });
                    }
                },
                on_cancel: move |_| confirm_revoke.set(None),
            }
        }

        // Revoke all devices for a user confirmation
        if let Some(uid) = confirm_revoke_user.read().clone() {
            ConfirmDialog {
                message: "Revoke ALL devices for this user? They will be signed out everywhere.",
                on_confirm: {
                    let client = app_state_revoke_all.client.clone();
                    move |_| {
                        let uid = uid.clone();
                        let client = client.clone();
                        let mut cru = confirm_revoke_user.clone();
                        let mut ref_ = refresh.clone();
                        let mut err = error.clone();
                        spawn(async move {
                            if let Ok(parsed) = uid.parse::<uuid::Uuid>() {
                                match client
                                    .execute(DeleteUserDevices { user_id: parsed })
                                    .await
                                {
                                    Ok(_) => {
                                        cru.set(None);
                                        let v = *ref_.peek() + 1;
                                        ref_.set(v);
                                    }
                                    Err(e) => {
                                        cru.set(None);
                                        err.set(Some(format!("Failed to revoke sessions: {e}")));
                                    }
                                }
                            }
                        });
                    }
                },
                on_cancel: move |_| confirm_revoke_user.set(None),
            }
        }
    }
}

