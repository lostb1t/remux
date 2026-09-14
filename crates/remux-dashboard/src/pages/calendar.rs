use crate::{
    components::{Card, EmptyState, LoadingText},
    state::{fmt_time, AppState},
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CalendarLinkInfo, CreateCalendarLink, DeleteCalendarLink, GetCalendarLinks,
    GetUsers, RotateCalendarLink, UserDto,
};
use uuid::Uuid;

/// Absolute feed URL for a server-relative path, so the value shown is the one
/// a calendar client can subscribe to directly.
fn absolute_url(path: &str) -> String {
    web_sys::window()
        .and_then(|w| {
            w.location()
                .origin()
                .ok()
        })
        .map(|origin| format!("{origin}{path}"))
        .unwrap_or_else(|| path.to_string())
}

#[component]
pub fn CalendarPage(app_state: AppState) -> Element {
    let mut links: Signal<Vec<CalendarLinkInfo>> = use_signal(Vec::new);
    let mut users: Signal<Vec<UserDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Generate dialog
    let mut show_generate = use_signal(|| false);
    let mut selected_user: Signal<Option<Uuid>> = use_signal(|| None);
    let mut generating = use_signal(|| false);

    // Confirm-revoke and confirm-rotate state, keyed by user.
    let mut to_revoke: Signal<Option<CalendarLinkInfo>> = use_signal(|| None);
    let mut to_rotate: Signal<Option<CalendarLinkInfo>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    let mut copied: Signal<Option<Uuid>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect.clone();
        spawn(async move {
            match client
                .execute(GetCalendarLinks)
                .await
            {
                Ok(result) => {
                    links.set(result);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load calendar links: {e}")))
                }
            }
            // Needed to offer a user picker and to label rows.
            if let Ok(result) = client
                .execute(GetUsers::default())
                .await
            {
                users.set(result);
            }
            loading.set(false);
        });
    });

    // Users without a link yet — the only sensible choices when generating.
    let linkable: Vec<UserDto> = {
        let existing: Vec<Uuid> = links
            .read()
            .iter()
            .map(|l| l.user_id)
            .collect();
        users
            .read()
            .iter()
            .filter(|u| !existing.contains(&u.id))
            .cloned()
            .collect()
    };

    let app_state_generate = app_state.clone();
    let app_state_rotate = app_state.clone();
    let app_state_revoke = app_state.clone();

    rsx! {
        Card {
            title: "Calendar",
            tight: true,
            action: rsx! {
                button {
                    class: "btn btn-primary",
                    // A green button that does nothing reads as enabled, so dim it
                    // when every user already has a link.
                    style: if linkable.is_empty() {
                        "height:32px;font-size:.68rem;opacity:.45;cursor:not-allowed"
                    } else {
                        "height:32px;font-size:.68rem"
                    },
                    disabled: linkable.is_empty(),
                    title: if linkable.is_empty() {
                        "Every user already has a link"
                    } else {
                        "Generate a calendar link for a user"
                    },
                    onclick: move |_| {
                        selected_user.set(None);
                        show_generate.set(true);
                    },
                    "+ Generate Link"
                }
            },
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "Subscribable calendar of release dates for the items a user follows: anything they favorited, plus upcoming episodes of series they have watched. Send the link to the user — anyone holding it can read that calendar, and it works without signing in."
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if links.read().is_empty() {
                EmptyState { message: "No calendar links — generate one to get started." }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for link in links.read().clone() {
                            {
                                let url = absolute_url(&link.url);
                                let created = fmt_time(
                                    link.created_at.format("%Y-%m-%d %H:%M"),
                                );
                                let rotated = link
                                    .rotated_at
                                    .map(|d| fmt_time(d.format("%Y-%m-%d %H:%M")));
                                let user_id = link.user_id;
                                let url_copy = url.clone();
                                let for_rotate = link.clone();
                                let for_revoke = link.clone();
                                let is_copied = *copied.read() == Some(user_id);
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                        key: "{user_id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem", "{link.user_name}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);font-family:monospace;margin-top:2px;word-break:break-all", "{url}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px",
                                                if let Some(rotated) = rotated {
                                                    "Created: {created} · Rotated: {rotated}"
                                                } else {
                                                    "Created: {created}"
                                                }
                                            }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    if let Some(clipboard) = web_sys::window().map(|w| w.navigator().clipboard()) {
                                                        let _ = clipboard.write_text(&url_copy);
                                                    }
                                                    copied.set(Some(user_id));
                                                },
                                                if is_copied { "Copied" } else { "Copy" }
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| to_rotate.set(Some(for_rotate.clone())),
                                                "Rotate"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| to_revoke.set(Some(for_revoke.clone())),
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

        if *show_generate.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Generate Calendar Link" }
                    }
                    div { class: "modal-body",
                        p { style: "color:var(--text-muted);font-size:.75rem;margin-bottom:10px",
                            "Pick the user this calendar belongs to. The feed shows only what they follow, filtered by their own library permissions."
                        }
                        select {
                            class: "form-input",
                            onchange: move |e| {
                                selected_user.set(e.value().parse().ok());
                            },
                            option { value: "", "Select a user…" }
                            for user in linkable.clone() {
                                option { value: "{user.id}", "{user.name}" }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| show_generate.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: selected_user.read().is_none() || *generating.read(),
                            onclick: move |_| {
                                let Some(user_id) = *selected_user.read() else { return };
                                let client = app_state_generate.clone();
                                generating.set(true);
                                spawn(async move {
                                    match client.execute(CreateCalendarLink { user_id }).await {
                                        Ok(_) => {
                                            show_generate.set(false);
                                            refresh.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => error.set(Some(format!("Failed to generate link: {e}"))),
                                    }
                                    generating.set(false);
                                });
                            },
                            if *generating.read() { "Generating…" } else { "Generate" }
                        }
                    }
                }
            }
        }

        if let Some(link) = to_rotate.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Rotate link for {link.user_name}?" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.78rem",
                            "This issues a new URL and stops the current one working. {link.user_name} will have to re-subscribe with the new link."
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| to_rotate.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *busy.read(),
                            onclick: move |_| {
                                let user_id = link.user_id;
                                let client = app_state_rotate.clone();
                                busy.set(true);
                                spawn(async move {
                                    match client.execute(RotateCalendarLink { user_id }).await {
                                        Ok(_) => {
                                            to_rotate.set(None);
                                            copied.set(None);
                                            refresh.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => error.set(Some(format!("Failed to rotate link: {e}"))),
                                    }
                                    busy.set(false);
                                });
                            },
                            if *busy.read() { "Rotating…" } else { "Rotate" }
                        }
                    }
                }
            }
        }

        if let Some(link) = to_revoke.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Revoke link for {link.user_name}?" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.78rem",
                            "The URL stops working immediately and their subscribed calendar will stop updating."
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| to_revoke.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            style: "background:var(--error);border-color:var(--error)",
                            disabled: *busy.read(),
                            onclick: move |_| {
                                let user_id = link.user_id;
                                let client = app_state_revoke.clone();
                                busy.set(true);
                                spawn(async move {
                                    match client.execute(DeleteCalendarLink { user_id }).await {
                                        Ok(_) => {
                                            to_revoke.set(None);
                                            refresh.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => error.set(Some(format!("Failed to revoke link: {e}"))),
                                    }
                                    busy.set(false);
                                });
                            },
                            if *busy.read() { "Revoking…" } else { "Revoke" }
                        }
                    }
                }
            }
        }
    }
}
