use crate::{components::*, pages::streams::StreamFilterEditor, state::AppState};
use dioxus::prelude::*;
use remux_sdks::remux::{
    AddonDto, AdminSetPassword, CollectionFilter, CreateUser, DeleteUser, FilterGroup,
    FilterMatchMode, GetUserAddons, GetUsers, ListAddons, SetUserAddons, StreamFilter,
    StreamRule, UpdateUser, UpdateUserPolicy, UserDto,
};
use uuid::Uuid;

#[derive(Clone)]
pub enum UserFormMode {
    Create,
    Edit(UserDto),
}

impl PartialEq for UserFormMode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Create, Self::Create) => true,
            (Self::Edit(a), Self::Edit(b)) => a.id == b.id,
            _ => false,
        }
    }
}

#[component]
pub fn UsersPage(app_state: AppState) -> Element {
    let mut users: Signal<Vec<UserDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);
    let mut form_mode: Signal<Option<UserFormMode>> = use_signal(|| None);

    // ID of the currently logged-in user (to disable self-delete)
    let self_id = app_state
        .server
        .user_id
        .clone();

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetUsers)
                .await
            {
                Ok(list) => {
                    users.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load users: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Users" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| form_mode.set(Some(UserFormMode::Create)),
                    "+ New User"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if users.read().is_empty() {
                    EmptyState { message: "No users found" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            for user in users.read().clone() {
                                {
                                    let is_self   = user.id.to_string() == self_id;
                                    let is_admin  = user.policy.is_administrator;
                                    let user_edit = user.clone();
                                    let user_id   = user.id;
                                    let client_del = app_state.client.clone();
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]", key: "{user.id}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "user-info",
                                                    span { class: "user-name", "{user.name}" }
                                                    if is_self {
                                                        span { class: "user-badge user-badge-self", "You" }
                                                    }
                                                    if is_admin {
                                                        span { class: "user-badge user-badge-admin", "Admin" }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px",
                                                    onclick: move |_| form_mode.set(Some(UserFormMode::Edit(user_edit.clone()))),
                                                    "Edit"
                                                }
                                                button {
                                                    class: "btn btn-ghost",
                                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                    disabled: is_self,
                                                    onclick: move |_| {
                                                        let c = client_del.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(DeleteUser { user_id }).await;
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                    "Delete"
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

        if let Some(mode) = form_mode.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    UserForm {
                        mode,
                        app_state: app_state.clone(),
                        on_done: move |_| {
                            form_mode.set(None);
                            let v = *refresh.peek() + 1;
                            refresh.set(v);
                        },
                        on_cancel: move |_| form_mode.set(None),
                    }
                }
            }
        }
    }
}

#[component]
pub fn UserForm(
    mode: UserFormMode,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let is_edit = matches!(mode, UserFormMode::Edit(_));
    let existing: Option<UserDto> = match &mode {
        UserFormMode::Edit(u) => Some(u.clone()),
        UserFormMode::Create => None,
    };

    let mut username = use_signal(|| {
        existing
            .as_ref()
            .map(|u| {
                u.name
                    .clone()
            })
            .unwrap_or_default()
    });
    let mut is_admin = use_signal(|| {
        existing
            .as_ref()
            .map(|u| {
                u.policy
                    .is_administrator
            })
            .unwrap_or(false)
    });
    let mut password = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut err = use_signal(|| Option::<String>::None);
    let fr_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| {
                u.policy
                    .filter_rules
                    .as_ref()
            })
            .map(|f| {
                f.match_mode
                    .clone()
            })
            .unwrap_or(FilterMatchMode::All)
    });
    let fr_groups: Signal<Vec<FilterGroup>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| {
                u.policy
                    .filter_rules
                    .as_ref()
            })
            .map(|f| {
                f.groups
                    .clone()
            })
            .unwrap_or_else(|| vec![FilterGroup::default()])
    });
    let sf_stream_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| {
                u.policy
                    .stream_filter
                    .as_ref()
            })
            .map(|f| {
                f.match_mode
                    .clone()
            })
            .unwrap_or(FilterMatchMode::All)
    });
    let sf_stream_rules: Signal<Vec<StreamRule>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|u| {
                u.policy
                    .stream_filter
                    .as_ref()
            })
            .map(|f| {
                f.rules
                    .clone()
            })
            .unwrap_or_default()
    });
    let mut enable_remote_search = use_signal(|| {
        existing
            .as_ref()
            .map(|u| {
                u.policy
                    .enable_remote_search
            })
            .unwrap_or(true)
    });
    let mut max_active_sessions: Signal<i64> = use_signal(|| {
        existing
            .as_ref()
            .map(|u| {
                u.policy
                    .max_active_sessions
            })
            .unwrap_or(0)
    });
    let mut enable_video_transcoding = use_signal(|| {
        existing
            .as_ref()
            .map(|u| {
                u.policy
                    .enable_video_playback_transcoding
            })
            .unwrap_or(true)
    });

    // Addon override state — edit only.
    // Each entry is (addon_id, enabled). Order is the user-defined priority.
    // None = no override (use default list).
    let mut all_addons: Signal<Vec<AddonDto>> = use_signal(Vec::new);
    let mut addon_override: Signal<Option<Vec<(Uuid, bool)>>> = use_signal(|| None);
    let edit_user_id = existing
        .as_ref()
        .map(|u| u.id);
    let addon_client = app_state
        .client
        .clone();
    use_effect(move || {
        let Some(uid) = edit_user_id else {
            return;
        };
        let c = addon_client.clone();
        spawn(async move {
            let (addons_res, override_res) = futures::join!(
                c.execute(ListAddons),
                c.execute(GetUserAddons { user_id: uid }),
            );
            if let Ok(ref a) = addons_res {
                all_addons.set(a.clone());
            }
            if let Ok(enabled_ids) = override_res {
                if !enabled_ids.is_empty() {
                    // Reconstruct full ordered list: enabled IDs in saved order,
                    // then any addons not in the saved list appended (disabled).
                    let addons = addons_res.unwrap_or_default();
                    let mut ordered: Vec<(Uuid, bool)> = enabled_ids
                        .iter()
                        .filter_map(|id| {
                            addons
                                .iter()
                                .find(|a| a.id == *id)
                                .map(|a| (a.id, true))
                        })
                        .collect();
                    // Addons not in the saved list: default/system ones start checked, non-default unchecked.
                    for a in &addons {
                        if !enabled_ids.contains(&a.id) {
                            ordered.push((a.id, false));
                        }
                    }
                    // Always ensure system addons are marked enabled.
                    for entry in ordered.iter_mut() {
                        if addons
                            .iter()
                            .any(|a| a.id == entry.0 && a.system)
                        {
                            entry.1 = true;
                        }
                    }
                    addon_override.set(Some(ordered));
                }
            }
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let pw = password
            .peek()
            .clone();
        let pw2 = password2
            .peek()
            .clone();
        if !pw.is_empty() && pw != pw2 {
            err.set(Some("Passwords do not match".into()));
            return;
        }
        if !is_edit && pw.is_empty() {
            err.set(Some("Password is required".into()));
            return;
        }

        let client = app_state
            .client
            .clone();
        let name = username
            .peek()
            .clone();
        let admin = *is_admin.peek();
        let user_dto = existing.clone();
        let groups_snapshot = fr_groups
            .peek()
            .clone();
        let match_snapshot = fr_match
            .peek()
            .clone();
        let stream_rules_snapshot = sf_stream_rules
            .peek()
            .clone();
        let stream_match_snapshot = sf_stream_match
            .peek()
            .clone();
        let remote_search_snapshot = *enable_remote_search.peek();
        let max_sessions_snapshot = *max_active_sessions.peek();
        let video_transcoding_snapshot = *enable_video_transcoding.peek();
        let addon_override_snapshot = addon_override
            .peek()
            .clone();

        saving.set(true);
        err.set(None);
        spawn(async move {
            let has_rules = groups_snapshot
                .iter()
                .any(|g| {
                    !g.rules
                        .is_empty()
                });
            let filter_rules = if has_rules {
                Some(CollectionFilter {
                    match_mode: match_snapshot,
                    groups: groups_snapshot,
                })
            } else {
                None
            };
            let stream_filter = if stream_rules_snapshot.is_empty() {
                None
            } else {
                Some(StreamFilter {
                    match_mode: stream_match_snapshot,
                    rules: stream_rules_snapshot,
                })
            };
            let result: Result<(), remux_sdks::ClientError> = async {
                if is_edit {
                    let user = user_dto
                        .as_ref()
                        .unwrap();
                    // Update username
                    let mut updated = user.clone();
                    updated.name = name;
                    client
                        .execute(UpdateUser {
                            user_id: user.id,
                            dto: updated,
                        })
                        .await?;
                    // Update admin flag and filter rules
                    let mut policy = user
                        .policy
                        .clone();
                    policy.is_administrator = admin;
                    policy.filter_rules = filter_rules.clone();
                    policy.stream_filter = stream_filter.clone();
                    policy.enable_remote_search = remote_search_snapshot;
                    policy.max_active_sessions = max_sessions_snapshot;
                    policy.enable_video_playback_transcoding =
                        video_transcoding_snapshot;
                    client
                        .execute(UpdateUserPolicy {
                            user_id: user.id,
                            policy,
                        })
                        .await?;
                    // Change password only if provided
                    if !pw.is_empty() {
                        client
                            .execute(AdminSetPassword {
                                user_id: user.id,
                                new_pw: pw,
                            })
                            .await?;
                    }
                    // Save addon override: send enabled IDs in order (empty = clear override).
                    let ids: Vec<Uuid> = addon_override_snapshot
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(_, enabled)| *enabled)
                        .map(|(id, _)| id)
                        .collect();
                    client
                        .execute(SetUserAddons {
                            user_id: user.id,
                            addon_ids: ids,
                        })
                        .await?;
                } else {
                    // Create user
                    let new_user = client
                        .execute(CreateUser { name, password: pw })
                        .await?;
                    if admin
                        || filter_rules.is_some()
                        || stream_filter.is_some()
                        || !remote_search_snapshot
                        || max_sessions_snapshot > 0
                        || !video_transcoding_snapshot
                    {
                        let mut policy = new_user
                            .policy
                            .clone();
                        policy.is_administrator = admin;
                        policy.filter_rules = filter_rules.clone();
                        policy.stream_filter = stream_filter.clone();
                        policy.enable_remote_search = remote_search_snapshot;
                        policy.max_active_sessions = max_sessions_snapshot;
                        policy.enable_video_playback_transcoding =
                            video_transcoding_snapshot;
                        client
                            .execute(UpdateUserPolicy {
                                user_id: new_user.id,
                                policy,
                            })
                            .await?;
                    }
                }
                Ok(())
            }
            .await;

            match result {
                Ok(_) => on_done.call(()),
                Err(e) => {
                    err.set(Some(e.user_message()));
                    saving.set(false);
                }
            }
        });
    };

    rsx! {
        p { class: "modal-title",
            if is_edit { "Edit User" } else { "New User" }
        }

        form {
            onsubmit: on_submit,
            style: "display:flex;flex-direction:column;gap:14px",

            div { class: "field",
                label { class: "field-label", r#for: "u-name", "Username" }
                input {
                    id: "u-name",
                    r#type: "text",
                    class: "field-input",
                    required: true,
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "u-pw",
                    if is_edit { "New Password" } else { "Password" }
                }
                input {
                    id: "u-pw",
                    r#type: "password",
                    class: "field-input",
                    required: !is_edit,
                    placeholder: if is_edit { "Leave blank to keep current" } else { "" },
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }
            }

            if !password.read().is_empty() || !is_edit {
                div { class: "field",
                    label { class: "field-label", r#for: "u-pw2", "Confirm Password" }
                    input {
                        id: "u-pw2",
                        r#type: "password",
                        class: "field-input",
                        required: !is_edit,
                        value: "{password2}",
                        oninput: move |e| password2.set(e.value()),
                    }
                }
            }

            ToggleRow {
                label: "Administrator",
                checked: *is_admin.read(),
                on_change: move |v| is_admin.set(v),
            }

            ToggleRow {
                label: "Allow Remote Search",
                checked: *enable_remote_search.read(),
                on_change: move |v| enable_remote_search.set(v),
            }

            ToggleRow {
                label: "Allow Video Transcoding",
                checked: *enable_video_transcoding.read(),
                on_change: move |v| enable_video_transcoding.set(v),
            }

            div { class: "field",
                label { class: "field-label", r#for: "u-max-streams", "Max Concurrent Streams" }
                input {
                    id: "u-max-streams",
                    r#type: "number",
                    class: "field-input",
                    min: "1",
                    placeholder: "Unlimited",
                    value: if *max_active_sessions.read() > 0 { max_active_sessions.read().to_string() } else { String::new() },
                    oninput: move |e| {
                        let v = e.value();
                        max_active_sessions.set(
                            v.parse::<i64>().map(|n| n.max(1)).unwrap_or(0)
                        );
                    },
                }
                span { class: "field-hint", "Leave blank for unlimited" }
            }

            if is_edit && !all_addons.read().is_empty() {
                div { class: "field",
                    div { class: "field-row",
                        label { class: "field-label", "Custom Addon List" }
                        input {
                            r#type: "checkbox",
                            checked: addon_override.read().is_some(),
                            onchange: move |e| {
                                if e.checked() {
                                    // Pre-check addons that are default (or system); non-default start unchecked.
                                    let entries = all_addons.read().iter()
                                        .map(|a| (a.id, a.is_default || a.system))
                                        .collect();
                                    addon_override.set(Some(entries));
                                } else {
                                    addon_override.set(None);
                                }
                            },
                        }
                    }
                    span { class: "field-hint",
                        "Override which addons run for this user and in what order. System addons always run regardless."
                    }
                    if addon_override.read().is_some() {
                        {
                            let entries = addon_override.read().clone().unwrap_or_default();
                            let total = entries.len();
                            rsx! {
                                div { style: "display:flex;flex-direction:column;gap:2px;margin-top:8px",
                                    for (idx, (aid, enabled)) in entries.iter().enumerate() {
                                        {
                                            let aid = *aid;
                                            let enabled = *enabled;
                                            let is_system = all_addons.read().iter()
                                                .find(|a| a.id == aid)
                                                .map(|a| a.system)
                                                .unwrap_or(false);
                                            let name = all_addons.read().iter()
                                                .find(|a| a.id == aid)
                                                .map(|a| a.name.clone())
                                                .unwrap_or_default();
                                            let kind = all_addons.read().iter()
                                                .find(|a| a.id == aid)
                                                .map(|a| a.kind.clone())
                                                .unwrap_or_default();
                                            rsx! {
                                                div {
                                                    key: "{aid}",
                                                    style: if enabled { "display:flex;align-items:center;gap:6px;padding:4px 0" } else { "display:flex;align-items:center;gap:6px;padding:4px 0;opacity:.4" },
                                                    if is_system {
                                                        span { style: "width:16px;text-align:center;font-size:.65rem;color:var(--text-dim)", "🔒" }
                                                    } else {
                                                        input {
                                                            r#type: "checkbox",
                                                            checked: enabled,
                                                            onchange: move |e| {
                                                                let mut ov = addon_override.write();
                                                                if let Some(ref mut list) = *ov {
                                                                    if let Some(entry) = list.iter_mut().find(|(id, _)| *id == aid) {
                                                                        entry.1 = e.checked();
                                                                    }
                                                                }
                                                            },
                                                        }
                                                    }
                                                    span { style: "flex:1", "{name}" }
                                                    span { class: "addon-card-kind", style: "font-size:.65rem", "{kind}" }
                                                    div { style: "display:flex;flex-direction:column;gap:1px",
                                                        button {
                                                            r#type: "button",
                                                            class: "btn btn-ghost",
                                                            style: "padding:0 4px;height:16px;font-size:.6rem;line-height:1",
                                                            disabled: idx == 0,
                                                            onclick: move |_| {
                                                                let mut ov = addon_override.write();
                                                                if let Some(ref mut list) = *ov {
                                                                    if idx > 0 { list.swap(idx - 1, idx); }
                                                                }
                                                            },
                                                            "↑"
                                                        }
                                                        button {
                                                            r#type: "button",
                                                            class: "btn btn-ghost",
                                                            style: "padding:0 4px;height:16px;font-size:.6rem;line-height:1",
                                                            disabled: idx == total - 1,
                                                            onclick: move |_| {
                                                                let mut ov = addon_override.write();
                                                                if let Some(ref mut list) = *ov {
                                                                    if idx < list.len() - 1 { list.swap(idx, idx + 1); }
                                                                }
                                                            },
                                                            "↓"
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
            }

            FilterRuleEditor {
                match_mode: fr_match,
                groups: fr_groups,
            }

            div { style: "margin-top:10px",
                StreamFilterEditor {
                    match_mode: sf_stream_match,
                    rules: sf_stream_rules,
                }
            }

            if let Some(e) = err.read().as_ref() {
                ErrorAlert { message: e.clone() }
            }

            FormActions {
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
                button {
                    r#type: "submit",
                    class: "btn btn-primary",
                    disabled: *saving.read(),
                    if *saving.read() { "Saving…" } else { "Save" }
                }
            }
        }
    }
}
