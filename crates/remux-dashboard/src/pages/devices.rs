use crate::{
    components::{Card, ConfirmDialog, EmptyState, ErrorAlert, LoadingText},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    DeleteDevice, DeleteUserDevices, DeviceInfo, GetDevices, QueryResult,
};
use std::collections::HashMap;

const DEFAULT_PAGE_SIZE: i64 = 25;

/// Groups devices from the current page by user, preserving server order.
/// Returns (uid, display_name, devices).
fn group_by_user(devices: &[DeviceInfo]) -> Vec<(String, String, Vec<DeviceInfo>)> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, (String, Vec<DeviceInfo>)> = HashMap::new();
    for d in devices {
        let uid = d
            .remux
            .as_ref()
            .and_then(|r| r.user_id)
            .map(|u| u.to_string())
            .unwrap_or_default();
        let label = d
            .last_user_name
            .clone()
            .unwrap_or_else(|| uid.clone());
        if !map.contains_key(&uid) {
            order.push(uid.clone());
            map.insert(uid.clone(), (label, vec![d.clone()]));
        } else if let Some(entry) = map.get_mut(&uid) {
            entry
                .1
                .push(d.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|uid| {
            map.remove(&uid)
                .map(|(name, devs)| (uid, name, devs))
        })
        .collect()
}

#[component]
pub fn DevicesPage(app_state: AppState) -> Element {
    let mut devices: Signal<Vec<DeviceInfo>> = use_signal(Vec::new);
    let mut total_count: Signal<i64> = use_signal(|| 0);
    let mut start_index: Signal<i64> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let refresh = use_signal(|| 0_u32);
    let mut confirm_revoke: Signal<Option<String>> = use_signal(|| None);
    let mut confirm_revoke_user: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut search_input: Signal<String> = use_signal(String::new);
    let mut page_size: Signal<i64> = use_signal(|| DEFAULT_PAGE_SIZE);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        let offset = *start_index.read();
        let limit = *page_size.read();
        let search = search_input
            .read()
            .clone();
        loading.set(true);
        let client = app_state_effect.clone();
        spawn(async move {
            let search_term = if search.is_empty() {
                None
            } else {
                Some(search)
            };
            match client
                .execute(GetDevices {
                    user_id: None,
                    start_index: Some(offset),
                    limit: Some(limit),
                    search_term,
                })
                .await
            {
                Ok(QueryResult {
                    items,
                    total_record_count,
                    ..
                }) => {
                    total_count.set(total_record_count);
                    devices.set(items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load devices: {e}"))),
            }
            loading.set(false);
        });
    });

    let app_state_revoke = app_state.clone();
    let app_state_revoke_all = app_state.clone();

    let total = *total_count.read();
    let offset = *start_index.read();
    let ps = *page_size.read();
    let has_prev = offset > 0;
    let has_next = offset + ps < total;

    let sections = group_by_user(&devices.read());

    rsx! {
        Card { title: "All Devices", tight: true,
            div { class: "device-search",
                input {
                    r#type: "text",
                    class: "input",
                    placeholder: "Filter by username…",
                    value: "{search_input.read()}",
                    oninput: move |evt| {
                        search_input.set(evt.value());
                        start_index.set(0);
                    },
                }
                if !search_input.read().is_empty() {
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;padding:0 8px;font-size:.75rem",
                        onclick: move |_| {
                            search_input.set(String::new());
                            start_index.set(0);
                        },
                        "×"
                    }
                }
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                ErrorAlert { message: err.clone() }
            } else if devices.read().is_empty() {
                EmptyState { message: "No devices found" }
            } else {
                div { class: "data-table-container",
                    for (uid, user_label, user_devices) in sections {
                        {
                            let uid_for_btn = uid.clone();
                            let label_for_btn = user_label.clone();
                            rsx! {
                                div { class: "device-user-section", key: "{uid}",
                                    div { class: "device-user-header",
                                        span { class: "device-user-label", "{user_label}" }
                                        button {
                                            class: "btn-section-revoke",
                                            onclick: move |_| confirm_revoke_user.set(Some((uid_for_btn.clone(), label_for_btn.clone()))),
                                            "Revoke all sessions"
                                        }
                                    }
                                    div { class: "row-list",
                                        for device in user_devices {
                                            {
                                                let device_id = device.id.clone().unwrap_or_default();
                                                let is_self = device
                                                    .remux
                                                    .as_ref()
                                                    .and_then(|r| r.is_current_session)
                                                    .unwrap_or(false);
                                                let device_id_revoke = device_id.clone();
                                                let remote_ip = device
                                                    .remux
                                                    .as_ref()
                                                    .and_then(|r| r.remote_end_point.clone());
                                                rsx! {
                                                    div { class: "device-row", key: "{device_id}",
                                                        div { class: "device-col-name",
                                                            div { class: "session-name",
                                                                "{device.name.as_deref().unwrap_or(\"Unknown device\")}"
                                                                if is_self {
                                                                    span {
                                                                        class: "user-badge user-badge-self",
                                                                        style: "margin-left:6px",
                                                                        "This session"
                                                                    }
                                                                }
                                                            }
                                                            div { style: "font-size:.7rem;color:var(--text-dim);margin-top:2px",
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
                                                        div { class: "device-col-app",
                                                            if let Some(app) = &device.app_name {
                                                                span { class: "session-client-badge",
                                                                    "{app}"
                                                                    if let Some(v) = &device.app_version {
                                                                        " {v}"
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        div { class: "device-col-time",
                                                            if let Some(t) = device.date_last_activity {
                                                                "{fmt_time(t)}"
                                                            }
                                                        }
                                                        div { class: "device-col-action",
                                                            button {
                                                                class: "btn-device-revoke",
                                                                disabled: is_self,
                                                                onclick: move |_| {
                                                                    confirm_revoke.set(Some(device_id_revoke.clone()))
                                                                },
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
                    }

                    div { class: "flex items-center justify-between px-3 py-2 border-t border-[var(--border)]",
                        span { class: "text-xs text-[var(--text-dim)]",
                            "{offset + 1}–{(offset + ps).min(total)} of {total}"
                        }
                        div { class: "flex items-center gap-2",
                            select {
                                class: "select-input",
                                style: "height:28px;font-size:.72rem;width:auto;padding:0 8px",
                                value: "{ps}",
                                onchange: move |evt| {
                                    if let Ok(v) = evt.value().parse::<i64>() {
                                        page_size.set(v);
                                        start_index.set(0);
                                    }
                                },
                                option { value: "25", selected: ps == 25, "25" }
                                option { value: "50", selected: ps == 50, "50" }
                                option { value: "100", selected: ps == 100, "100" }
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.72rem;padding:0 10px",
                                disabled: !has_prev,
                                onclick: move |_| start_index.set((offset - ps).max(0)),
                                "← Prev"
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.72rem;padding:0 10px",
                                disabled: !has_next,
                                onclick: move |_| start_index.set(offset + ps),
                                "Next →"
                            }
                        }
                    }
                }
            }
        }

        if let Some(did) = confirm_revoke.read().clone() {
            ConfirmDialog {
                message: "Revoke this device? It will be signed out immediately.",
                on_confirm: {
                    let client = app_state_revoke.clone();
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

        if let Some((uid, user_label)) = confirm_revoke_user.read().clone() {
            ConfirmDialog {
                message: "Revoke ALL sessions for {user_label}? They will be signed out everywhere.",
                on_confirm: {
                    let client = app_state_revoke_all.clone();
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
