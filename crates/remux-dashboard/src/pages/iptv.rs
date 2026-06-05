use crate::{
    components::{FormGroup, LoadingText, Modal},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    BulkChannelRequest, BulkChannels, ChannelEditorItem, GetIptvChannelCountries,
    GetIptvChannelGroups, GetIptvChannels, PatchChannel, PatchChannelRequest,
};

pub const PAGE_SIZE: u32 = 50;

#[component]
pub fn IptvPage(app_state: AppState) -> Element {
    rsx! {
        IptvChannelsTab { app_state }
    }
}

#[component]
pub(crate) fn IptvChannelsTab(app_state: AppState) -> Element {
    let mut channels: Signal<Vec<ChannelEditorItem>> = use_signal(Vec::new);
    let mut total: Signal<usize> = use_signal(|| 0);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut page = use_signal(|| 0_u32);
    // committed search (triggers fetch); typed search (live input)
    let mut search_committed = use_signal(String::new);
    let mut search_input = use_signal(String::new);
    let mut bulk_working = use_signal(|| false);
    // "all" | "true" | "false"
    let mut enabled_filter = use_signal(|| "all".to_string());
    let mut country_filter = use_signal(String::new);
    let mut countries: Signal<Vec<String>> = use_signal(Vec::new);
    let mut group_filter = use_signal(String::new);
    let mut groups: Signal<Vec<String>> = use_signal(Vec::new);
    // "order" | "name"
    let mut sort_mode = use_signal(|| "order".to_string());
    let mut show_filter_modal = use_signal(|| false);

    // Load distinct country codes and groups once on mount
    let app_state_countries = app_state.clone();
    use_effect(move || {
        let client = app_state_countries
            .client
            .clone();
        spawn(async move {
            if let Ok(cs) = client
                .execute(GetIptvChannelCountries)
                .await
            {
                countries.set(cs);
            }
            if let Ok(gs) = client
                .execute(GetIptvChannelGroups)
                .await
            {
                groups.set(gs);
            }
        });
    });

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let p = *page.read();
        let s = search_committed
            .read()
            .clone();
        let ef = enabled_filter
            .read()
            .clone();
        let cf = country_filter
            .read()
            .clone();
        let gf = group_filter
            .read()
            .clone();
        let sm = sort_mode
            .read()
            .clone();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            let enabled = match ef.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            match client
                .execute(GetIptvChannels {
                    limit: PAGE_SIZE,
                    offset: p * PAGE_SIZE,
                    search: s,
                    enabled,
                    country: cf,
                    group: gf,
                    sort: sm,
                })
                .await
            {
                Ok(r) => {
                    total.set(r.total_record_count);
                    channels.set(r.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load channels: {e}"))),
            }
            loading.set(false);
        });
    });

    let total_v = *total.read();
    let page_v = *page.read();
    let total_pages = total_v.div_ceil(PAGE_SIZE as usize) as u32;

    let mut do_search = move || {
        let s = search_input
            .peek()
            .clone();
        search_committed.set(s);
        page.set(0);
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Channels" }
                if total_v > 0 {
                    span { style: "font-size:.75rem;opacity:.5;margin-left:8px", "{total_v} total" }
                }
                div { style: "display:flex;gap:8px;align-items:center;margin-left:auto",
                    // Filters / Sort modal trigger
                    {
                        let filters_active = enabled_filter.read().as_str() != "all"
                            || !country_filter.read().is_empty()
                            || !group_filter.read().is_empty()
                            || sort_mode.read().as_str() != "order"
                            || !search_committed.read().is_empty();
                        rsx! {
                            button {
                                class: if filters_active { "btn btn-primary" } else { "btn btn-ghost" },
                                style: "height:32px;font-size:.68rem",
                                onclick: move |_| show_filter_modal.set(true),
                                if filters_active { "Filters ●" } else { "Filters" }
                            }
                        }
                    }
                    // Enable all / Disable all — server-side bulk op
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        disabled: *bulk_working.read() || total_v == 0,
                        onclick: {
                            let client = app_state.client.clone();
                            move |_| {
                                let search = search_committed.peek().clone();
                                bulk_working.set(true);
                                let c = client.clone();
                                spawn(async move {
                                    let _ = c.execute(BulkChannels {
                                        request: BulkChannelRequest { enabled: true, search: if search.is_empty() { None } else { Some(search) } },
                                    }).await;
                                    bulk_working.set(false);
                                    // re-fetch to reflect new state
                                    let s = search_committed.peek().clone();
                                    let p = *page.peek();
                                    let ef = enabled_filter.peek().clone();
                                    let cf = country_filter.peek().clone();
                                    let gf = group_filter.peek().clone();
                                    let sm = sort_mode.peek().clone();
                                    let enabled = match ef.as_str() {
                                        "true" => Some(true),
                                        "false" => Some(false),
                                        _ => None,
                                    };
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s, enabled, country: cf, group: gf, sort: sm }).await {
                                        total.set(r.total_record_count);
                                        channels.set(r.items);
                                    }
                                    loading.set(false);
                                });
                            }
                        },
                        if *bulk_working.read() { "Working…" } else { "Enable All" }
                    }
                    button {
                        class: "btn btn-ghost",
                        style: "height:32px;font-size:.68rem",
                        disabled: *bulk_working.read() || total_v == 0,
                        onclick: {
                            let client = app_state.client.clone();
                            move |_| {
                                let search = search_committed.peek().clone();
                                bulk_working.set(true);
                                let c = client.clone();
                                spawn(async move {
                                    let _ = c.execute(BulkChannels {
                                        request: BulkChannelRequest { enabled: false, search: if search.is_empty() { None } else { Some(search) } },
                                    }).await;
                                    bulk_working.set(false);
                                    let s = search_committed.peek().clone();
                                    let p = *page.peek();
                                    let ef = enabled_filter.peek().clone();
                                    let cf = country_filter.peek().clone();
                                    let gf = group_filter.peek().clone();
                                    let sm = sort_mode.peek().clone();
                                    let enabled = match ef.as_str() {
                                        "true" => Some(true),
                                        "false" => Some(false),
                                        _ => None,
                                    };
                                    loading.set(true);
                                    if let Ok(r) = c.execute(GetIptvChannels { limit: PAGE_SIZE, offset: p * PAGE_SIZE, search: s, enabled, country: cf, group: gf, sort: sm }).await {
                                        total.set(r.total_record_count);
                                        channels.set(r.items);
                                    }
                                    loading.set(false);
                                });
                            }
                        },
                        if *bulk_working.read() { "Working…" } else { "Disable All" }
                    }
                }
            }

            div { class: "card-body tight",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if channels.read().is_empty() {
                    div { class: "empty-state",
                        if total_v == 0
                            && search_committed.read().is_empty()
                            && enabled_filter.read().as_str() == "all"
                            && country_filter.read().is_empty()
                            && group_filter.read().is_empty()
                        {
                            "No channels yet. Run a refresh after adding channel sources."
                        } else {
                            "No channels match your filters."
                        }
                    }
                } else {
                    div { class: "data-table-container",
                        // Column header
                        div { class: "flex items-center px-3 py-1 border-b border-[var(--border)]",
                            style: "font-size:.72rem;opacity:.5;font-weight:600;gap:8px",
                            div { style: "width:32px", "On" }
                            div { class: "flex-1", "Name / Display Name" }
                            div { style: "width:140px", "Group" }
                            div { style: "width:80px;text-align:right", "Ch#" }
                        }
                        div { class: "row-list",
                            for ch in channels.read().clone() {
                                {
                                    let id = ch.id.clone();
                                    let client1 = app_state.client.clone();
                                    let client2 = app_state.client.clone();
                                    let client3 = app_state.client.clone();
                                    let sort_val = ch.sort_order.map(|n| n.to_string()).unwrap_or_default();
                                    let ch_placeholder = ch.channel_number.map(|n| n.to_string()).unwrap_or_else(|| "–".into());
                                    let name_val = ch.custom_name.clone().unwrap_or_default();

                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                            style: if !ch.enabled { "gap:8px;padding:6px 12px;opacity:.4" } else { "gap:8px;padding:6px 12px" },

                                            input {
                                                r#type: "checkbox",
                                                checked: ch.enabled,
                                                style: "width:16px;height:16px;cursor:pointer;flex-shrink:0",
                                                onchange: {
                                                    let id = id.clone();
                                                    move |e| {
                                                        let enabled = e.value() == "true";
                                                        // optimistic update
                                                        if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                            c.enabled = enabled;
                                                        }
                                                        let c = client1.clone();
                                                        let id = id.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(PatchChannel {
                                                                id,
                                                                patch: PatchChannelRequest { enabled: Some(enabled), ..Default::default() },
                                                            }).await;
                                                        });
                                                    }
                                                },
                                            }
                                            div { class: "flex-1 min-w-0",
                                                div { style: "font-size:.82rem;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis",
                                                    "{ch.name}"
                                                }
                                                input {
                                                    class: "form-input",
                                                    style: "height:24px;font-size:.75rem;padding:2px 6px;margin-top:2px;width:100%",
                                                    value: "{name_val}",
                                                    placeholder: "Custom display name…",
                                                    onchange: {
                                                        let id = id.clone();
                                                        move |e| {
                                                            let v = e.value();
                                                            let custom = if v.is_empty() { None } else { Some(v.clone()) };
                                                            if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                                c.custom_name = custom.clone();
                                                            }
                                                            let c = client3.clone();
                                                            let id = id.clone();
                                                            spawn(async move {
                                                                let _ = c.execute(PatchChannel {
                                                                    id,
                                                                    patch: PatchChannelRequest { custom_name: custom, ..Default::default() },
                                                                }).await;
                                                            });
                                                        }
                                                    },
                                                }
                                            }
                                            div { style: "width:140px;font-size:.75rem;opacity:.7;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex-shrink:0",
                                                {ch.group.as_deref().unwrap_or("–")}
                                            }
                                            input {
                                                class: "form-input",
                                                r#type: "number",
                                                style: "width:80px;height:28px;font-size:.8rem;padding:2px 6px;flex-shrink:0;text-align:right",
                                                value: "{sort_val}",
                                                placeholder: "{ch_placeholder}",
                                                onchange: {
                                                    let id = id.clone();
                                                    move |e| {
                                                        let v = e.value().parse::<i64>().ok();
                                                        if let Some(c) = channels.write().iter_mut().find(|c| c.id == id) {
                                                            c.sort_order = v;
                                                        }
                                                        let c = client2.clone();
                                                        let id = id.clone();
                                                        spawn(async move {
                                                            let _ = c.execute(PatchChannel {
                                                                id,
                                                                patch: PatchChannelRequest { sort_order: v, ..Default::default() },
                                                            }).await;
                                                        });
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Pagination bar
                    if total_pages > 1 {
                        div { class: "pagination-bar",
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.75rem",
                                disabled: page_v == 0,
                                onclick: move |_| page.set(page_v.saturating_sub(1)),
                                "‹ Prev"
                            }
                            span { style: "font-size:.8rem;opacity:.7",
                                "Page {page_v + 1} of {total_pages}"
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "height:28px;font-size:.75rem",
                                disabled: page_v + 1 >= total_pages,
                                onclick: move |_| page.set(page_v + 1),
                                "Next ›"
                            }
                        }
                    }
                }
            }
        }

        // Filter / Sort modal
        if *show_filter_modal.read() {
            Modal {
                on_close: move |_| show_filter_modal.set(false),
                div { class: "modal-header",
                    span { class: "modal-title", "Filters & Sort" }
                }
                div { class: "modal-body",
                    FormGroup { label: "Search",
                        input {
                            class: "form-input",
                            r#type: "text",
                            placeholder: "Search channels…",
                            value: "{search_input.read()}",
                            oninput: move |e| search_input.set(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Enter {
                                    do_search();
                                    show_filter_modal.set(false);
                                }
                            },
                        }
                    }
                    FormGroup { label: "Sort by",
                        select {
                            class: "form-input",
                            value: "{sort_mode.read()}",
                            onchange: move |e| { sort_mode.set(e.value()); page.set(0); },
                            option { value: "order", "Order" }
                            option { value: "name", "Name" }
                        }
                    }
                    FormGroup { label: "Status",
                        select {
                            class: "form-input",
                            value: "{enabled_filter.read()}",
                            onchange: move |e| { enabled_filter.set(e.value()); page.set(0); },
                            option { value: "all", "All" }
                            option { value: "true", "Enabled" }
                            option { value: "false", "Disabled" }
                        }
                    }
                    if !countries.read().is_empty() {
                        FormGroup { label: "Country",
                            select {
                                class: "form-input",
                                value: "{country_filter.read()}",
                                onchange: move |e| { country_filter.set(e.value()); page.set(0); },
                                option { value: "", "All countries" }
                                for c in countries.read().clone() {
                                    option { value: "{c}", "{c}" }
                                }
                            }
                        }
                    }
                    if !groups.read().is_empty() {
                        FormGroup { label: "Group",
                            select {
                                class: "form-input",
                                value: "{group_filter.read()}",
                                onchange: move |e| { group_filter.set(e.value()); page.set(0); },
                                option { value: "", "All groups" }
                                for g in groups.read().clone() {
                                    option { value: "{g}", "{g}" }
                                }
                            }
                        }
                    }
                }
                div { class: "modal-footer",
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| {
                            search_input.set(String::new());
                            search_committed.set(String::new());
                            enabled_filter.set("all".to_string());
                            country_filter.set(String::new());
                            group_filter.set(String::new());
                            sort_mode.set("order".to_string());
                            page.set(0);
                        },
                        "Reset"
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            do_search();
                            show_filter_modal.set(false);
                        },
                        "Done"
                    }
                }
            }
        }
    }
}
