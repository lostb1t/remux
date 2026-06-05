use crate::{
    components::{EmptyState, FormGroup, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::{
    remux::{
        AddonCatalogDto, AddonDto, AddonMetadata, AddonOption, AddonOptionType,
        AddonPresetRef, CreateAddon, CreateAddonRequest, DeleteAddon, GetAddonCatalogs,
        ListAddonKinds, ListAddons, UpdateAddon, UpdateAddonCatalogRequest,
        UpdateAddonCatalogs, UpdateAddonRequest,
    },
    stremio::ResourceType,
};
use uuid::Uuid;

#[component]
pub fn AddonsPage(app_state: AppState) -> Element {
    let mut addons: Signal<Vec<AddonDto>> = use_signal(Vec::new);
    let mut kinds: Signal<Vec<AddonMetadata>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0_u32);

    // Add-addon modal state
    let mut show_create = use_signal(|| false);
    let mut create_step: Signal<u8> = use_signal(|| 0); // 0 = pick kind, 1 = configure
    let mut selected_kind: Signal<Option<String>> = use_signal(|| None);
    let mut name_input = use_signal(String::new);
    // Form values keyed by option id; stored as serde_json::Value to round-trip cleanly.
    let mut form_values: Signal<std::collections::HashMap<String, serde_json::Value>> =
        use_signal(std::collections::HashMap::new);
    let mut creating = use_signal(|| false);

    // Edit-addon modal state
    let mut id_to_edit: Signal<Option<Uuid>> = use_signal(|| None);
    let mut edit_name_input = use_signal(String::new);
    let mut edit_form_values: Signal<
        std::collections::HashMap<String, serde_json::Value>,
    > = use_signal(std::collections::HashMap::new);
    let mut editing = use_signal(|| false);
    // Resources checked state for edit form (set of enabled ResourceType display strings)
    let mut edit_resources: Signal<std::collections::HashSet<String>> =
        use_signal(std::collections::HashSet::new);
    // Types checked state for edit form (set of enabled MediaKind display strings)
    let mut edit_types: Signal<std::collections::HashSet<String>> =
        use_signal(std::collections::HashSet::new);
    // Catalogs loaded for the addon being edited
    let mut edit_catalogs: Signal<Vec<AddonCatalogDto>> = use_signal(Vec::new);
    let mut edit_catalogs_loading = use_signal(|| false);
    // Per-catalog overrides: catalog_id -> (enabled, max_items_str, tags_str, create_collection)
    let mut edit_catalog_settings: Signal<
        std::collections::HashMap<String, (bool, String, String, bool)>,
    > = use_signal(std::collections::HashMap::new);

    // Confirm-delete state
    let mut id_to_delete: Signal<Option<Uuid>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            let kinds_res = client
                .execute(ListAddonKinds)
                .await;
            let addons_res = client
                .execute(ListAddons)
                .await;
            match (kinds_res, addons_res) {
                (Ok(k), Ok(a)) => {
                    kinds.set(k);
                    addons.set(a);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load addons: {e}")));
                }
            }
            loading.set(false);
        });
    });

    let selected_kind_meta = {
        let sel = selected_kind
            .read()
            .clone();
        sel.and_then(|id| {
            kinds
                .read()
                .iter()
                .find(|k| k.id == id)
                .cloned()
        })
    };

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Addons" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        name_input.set(String::new());
                        form_values.set(std::collections::HashMap::new());
                        selected_kind.set(None);
                        create_step.set(0);
                        show_create.set(true);
                    },
                    "+ New Addon"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if addons.read().is_empty() {
                    EmptyState { message: "No addons configured — add one to get started." }
                } else {
                    div { class: "addon-list",
                        for (addon_idx, addon) in addons.read().clone().into_iter().enumerate() {
                            {
                                let id = addon.id;
                                let addon_count = addons.read().len();
                                rsx! {
                                    div { class: "addon-card", key: "{id}",
                                        div { class: "addon-card-header",
                                            span { class: "addon-card-name", "{addon.name}" }
                                            span { class: "addon-card-kind", "{addon.kind}" }
                                        }
                                        div { class: "addon-kind-card-badges",
                                            for res in addon.resources.iter() {
                                                span { class: "addon-kind-badge", "{res:?}" }
                                            }
                                            {
                                                let display_types = if addon.types.is_empty() { &addon.supported_types } else { &addon.types };
                                                rsx! {
                                                    for t in display_types.iter() {
                                                        span { class: "addon-kind-type", "{t}" }
                                                    }
                                                }
                                            }
                                        }
                                        div { class: "addon-card-actions",
                                            // Up/down reorder buttons
                                            div { class: "addon-card-sort",
                                                button {
                                                    class: "btn btn-ghost addon-sort-btn",
                                                    disabled: addon_idx == 0,
                                                    title: "Move up (higher priority)",
                                                    onclick: {
                                                        let client = app_state.client.clone();
                                                        move |_| {
                                                            let current = addons.read().clone();
                                                            if addon_idx == 0 { return; }
                                                            let mut new_order = current.clone();
                                                            new_order.swap(addon_idx, addon_idx - 1);
                                                            let updates: Vec<(Uuid, i64)> = new_order.iter().enumerate()
                                                                .filter_map(|(i, a)| {
                                                                    let new_prio = i as i64 * 10;
                                                                    if a.priority != new_prio { Some((a.id, new_prio)) } else { None }
                                                                })
                                                                .collect();
                                                            let c = client.clone();
                                                            spawn(async move {
                                                                for (uid, prio) in updates {
                                                                    let _ = c.execute(UpdateAddon { id: uid, payload: UpdateAddonRequest { priority: Some(prio), ..Default::default() } }).await;
                                                                }
                                                                let v = *refresh.peek() + 1;
                                                                refresh.set(v);
                                                            });
                                                        }
                                                    },
                                                    "↑"
                                                }
                                                button {
                                                    class: "btn btn-ghost addon-sort-btn",
                                                    disabled: addon_idx + 1 >= addon_count,
                                                    title: "Move down (lower priority)",
                                                    onclick: {
                                                        let client = app_state.client.clone();
                                                        move |_| {
                                                            let current = addons.read().clone();
                                                            if addon_idx + 1 >= current.len() { return; }
                                                            let mut new_order = current.clone();
                                                            new_order.swap(addon_idx, addon_idx + 1);
                                                            let updates: Vec<(Uuid, i64)> = new_order.iter().enumerate()
                                                                .filter_map(|(i, a)| {
                                                                    let new_prio = i as i64 * 10;
                                                                    if a.priority != new_prio { Some((a.id, new_prio)) } else { None }
                                                                })
                                                                .collect();
                                                            let c = client.clone();
                                                            spawn(async move {
                                                                for (uid, prio) in updates {
                                                                    let _ = c.execute(UpdateAddon { id: uid, payload: UpdateAddonRequest { priority: Some(prio), ..Default::default() } }).await;
                                                                }
                                                                let v = *refresh.peek() + 1;
                                                                refresh.set(v);
                                                            });
                                                        }
                                                    },
                                                    "↓"
                                                }
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:28px;font-size:.68rem;padding:0 10px",
                                                onclick: {
                                                    let client = app_state.client.clone();
                                                    move |_| {
                                                        if let Some(a) = addons.read().iter().find(|a| a.id == id).cloned() {
                                                            edit_name_input.set(a.name.clone());
                                                            let config_map = a.config.as_object()
                                                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                                                .unwrap_or_default();
                                                            edit_form_values.set(config_map);
                                                            let res_set: std::collections::HashSet<String> = a.resources
                                                                .iter()
                                                                .map(|r| format!("{r}"))
                                                                .collect();
                                                            edit_resources.set(res_set);
                                                            // Empty types = all enabled — pre-check every supported type.
                                                            let type_set: std::collections::HashSet<String> = if a.types.is_empty() {
                                                                a.supported_types.iter().map(|t| format!("{t}")).collect()
                                                            } else {
                                                                a.types.iter().map(|t| format!("{t}")).collect()
                                                            };
                                                            edit_types.set(type_set);
                                                            let has_catalog = a.resources.contains(&ResourceType::Catalog);
                                                            edit_catalogs.set(Vec::new());
                                                            edit_catalog_settings.set(std::collections::HashMap::new());
                                                            id_to_edit.set(Some(id));
                                                            if has_catalog {
                                                                edit_catalogs_loading.set(true);
                                                                let c = client.clone();
                                                                spawn(async move {
                                                                    match c.execute(GetAddonCatalogs { id }).await {
                                                                        Ok(cats) => {
                                                                            let settings: std::collections::HashMap<String, (bool, String, String, bool)> = cats
                                                                                .iter()
                                                                                .map(|cat| (
                                                                                    cat.catalog_id.clone(),
                                                                                    (
                                                                                        cat.enabled,
                                                                                        cat.max_items.map(|n| n.to_string()).unwrap_or_default(),
                                                                                        cat.tags.join(", "),
                                                                                        cat.create_collection,
                                                                                    ),
                                                                                ))
                                                                                .collect();
                                                                            edit_catalog_settings.set(settings);
                                                                            edit_catalogs.set(cats);
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(format!("Failed to load catalogs: {e}")));
                                                                        }
                                                                    }
                                                                    edit_catalogs_loading.set(false);
                                                                });
                                                            }
                                                        }
                                                    }
                                                },
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:28px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| id_to_delete.set(Some(id)),
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

        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal modal--wide",
                    div { class: "modal-header",
                        span { class: "modal-title",
                            if *create_step.read() == 0 { "Choose Type" } else { "Configure Addon" }
                        }
                    }
                    div { class: "modal-body",
                        if *create_step.read() == 0 {
                            // ── Step 1: kind picker ──
                            div { class: "addon-kind-list",
                                for k in kinds.read().clone() {
                                    {
                                        let k_id = k.id.clone();
                                        let k_name = k.display_name.clone();
                                        let is_selected = selected_kind.read().as_deref() == Some(&k.id);
                                        rsx! {
                                            div {
                                                class: if is_selected { "addon-kind-card addon-kind-card--selected" } else { "addon-kind-card" },
                                                onclick: move |_| {
                                                    selected_kind.set(Some(k_id.clone()));
                                                    form_values.set(std::collections::HashMap::new());
                                                },
                                                div { class: "addon-kind-card-name", "{k.display_name}" }
                                                div { class: "addon-kind-card-desc", "{k.description}" }
                                                div { class: "addon-kind-card-badges",
                                                    for res in k.supported_resources.iter() {
                                                        span { class: "addon-kind-badge", "{res:?}" }
                                                    }
                                                    for t in k.supported_types.iter() {
                                                        span { class: "addon-kind-type", "{t}" }
                                                    }
                                                }
                                                if is_selected {
                                                    button {
                                                        class: "btn btn-primary addon-kind-card-configure",
                                                        onclick: move |e| {
                                                            e.stop_propagation();
                                                            name_input.set(k_name.clone());
                                                            create_step.set(1);
                                                        },
                                                        "Configure →"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // ── Step 2: name + options ──
                            if let Some(meta) = &selected_kind_meta {
                                div { class: "field-hint", style: "margin-bottom:4px", "{meta.description}" }
                            }
                            FormGroup { label: "Name",
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "Display name",
                                    value: "{name_input}",
                                    oninput: move |e| name_input.set(e.value()),
                                }
                            }
                            if let Some(meta) = &selected_kind_meta {
                                for opt in meta.options.iter().cloned() {
                                    AddonOptionField {
                                        option: opt,
                                        values: form_values,
                                    }
                                }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| {
                                if *create_step.read() == 1 {
                                    create_step.set(0);
                                } else {
                                    show_create.set(false);
                                }
                            },
                            if *create_step.read() == 1 { "← Back" } else { "Cancel" }
                        }
                        if *create_step.read() == 1 {
                            button {
                                class: "btn btn-primary",
                                disabled: *creating.read() || name_input.read().trim().is_empty() || selected_kind.read().is_none(),
                                onclick: {
                                    let client = app_state.client.clone();
                                    move |_| {
                                        let name = name_input.read().trim().to_string();
                                        let Some(kind) = selected_kind.read().clone() else { return; };
                                        if name.is_empty() { return; }
                                        let config: serde_json::Value = serde_json::Value::Object(
                                            form_values.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                        );
                                        creating.set(true);
                                        let c = client.clone();
                                        spawn(async move {
                                            let payload = CreateAddonRequest {
                                                preset: AddonPresetRef { kind, config },
                                                name,
                                                resources: Vec::new(),
                                                types: Vec::new(),
                                                priority: 0,
                                            };
                                            match c.execute(CreateAddon { payload }).await {
                                                Ok(_) => {
                                                    show_create.set(false);
                                                    let v = *refresh.peek() + 1;
                                                    refresh.set(v);
                                                }
                                                Err(e) => {
                                                    error.set(Some(format!("Failed to create addon: {e}")));
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
        }

        if let Some(edit_id) = *id_to_edit.read() {
            {
                let edit_kind = addons.read().iter().find(|a| a.id == edit_id).map(|a| a.kind.clone());
                let edit_kind_meta = edit_kind.as_ref().and_then(|k| kinds.read().iter().find(|m| m.id == *k).cloned());
                // Use supported_resources from the addon row (manifest-derived for Stremio,
                // kind-static for others) as the checkbox option list.
                let resource_options: Vec<ResourceType> = addons
                    .read()
                    .iter()
                    .find(|a| a.id == edit_id)
                    .map(|a| a.supported_resources.clone())
                    .unwrap_or_default();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "Edit Addon" }
                            }
                            div { class: "modal-body",
                                FormGroup { label: "Name",
                                    input {
                                        class: "form-input",
                                        r#type: "text",
                                        placeholder: "Display name",
                                        value: "{edit_name_input}",
                                        oninput: move |e| edit_name_input.set(e.value()),
                                    }
                                }
                                if let Some(meta) = &edit_kind_meta {
                                    for opt in meta.options.iter().cloned() {
                                        AddonOptionField {
                                            option: opt,
                                            values: edit_form_values,
                                        }
                                    }
                                }
                                // Resources section — options come from the addon row.
                                if !resource_options.is_empty() {
                                    div { class: "form-group",
                                        label { class: "form-label", "Resources" }
                                        div { class: "check-row-group",
                                            for res in resource_options.iter().cloned() {
                                                {
                                                    let res_str = format!("{res}");
                                                    let res_str_check = res_str.clone();
                                                    let checked = edit_resources.read().contains(&res_str);
                                                    rsx! {
                                                        label { class: "check-row",
                                                            input {
                                                                r#type: "checkbox",
                                                                checked,
                                                                onchange: move |e| {
                                                                    let mut set = edit_resources.write();
                                                                    if e.checked() {
                                                                        set.insert(res_str_check.clone());
                                                                    } else {
                                                                        set.remove(&res_str_check);
                                                                    }
                                                                },
                                                            }
                                                            "{res_str}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                // Types section
                                {
                                    let type_options: Vec<remux_sdks::remux::MediaKind> = addons
                                        .read()
                                        .iter()
                                        .find(|a| a.id == edit_id)
                                        .map(|a| a.supported_types.clone())
                                        .unwrap_or_default();
                                    if !type_options.is_empty() {
                                        rsx! {
                                            div { class: "form-group",
                                                label { class: "form-label", "Content Types" }
                                                div { class: "check-row-group",
                                                    for t in type_options.into_iter() {
                                                        {
                                                            let t_str = format!("{t}");
                                                            let t_str_check = t_str.clone();
                                                            let checked = edit_types.read().contains(&t_str);
                                                            rsx! {
                                                                label { class: "check-row",
                                                                    input {
                                                                        r#type: "checkbox",
                                                                        checked,
                                                                        onchange: move |e| {
                                                                            let mut set = edit_types.write();
                                                                            if e.checked() {
                                                                                set.insert(t_str_check.clone());
                                                                            } else {
                                                                                set.remove(&t_str_check);
                                                                            }
                                                                        },
                                                                    }
                                                                    "{t_str}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        rsx! {}
                                    }
                                }
                                // Catalogs section (only shown when catalog resource is active)
                                if edit_resources.read().contains("catalog") {
                                    div { class: "form-group",
                                        label { class: "form-label", "Catalogs" }
                                        if *edit_catalogs_loading.read() {
                                            span { class: "field-hint", "Loading catalogs…" }
                                        } else if edit_catalogs.read().is_empty() {
                                            span { class: "field-hint", "No catalogs found." }
                                        } else {
                                            div { class: "catalog-table-wrap",
                                                table { class: "catalog-table",
                                                    thead {
                                                        tr {
                                                            th { "Catalog" }
                                                            th { "Enabled" }
                                                            th { "Max items" }
                                                            th { "Tags" }
                                                            th { "Collection" }
                                                        }
                                                    }
                                                    tbody {
                                                        for cat in edit_catalogs.read().clone() {
                                                            {
                                                                let cid = cat.catalog_id.clone();
                                                                let cid_toggle = cid.clone();
                                                                let cid_max = cid.clone();
                                                                let cid_tags = cid.clone();
                                                                let cid_coll = cid.clone();
                                                                let (enabled, max_str, tags_str, create_coll) = edit_catalog_settings.read()
                                                                    .get(&cid)
                                                                    .cloned()
                                                                    .unwrap_or((false, String::new(), String::new(), false));
                                                                rsx! {
                                                                    tr {
                                                                        td { class: "catalog-name", "{cat.name}" }
                                                                        td {
                                                                            input {
                                                                                r#type: "checkbox",
                                                                                checked: enabled,
                                                                                onchange: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_toggle.clone()).or_default();
                                                                                    entry.0 = e.checked();
                                                                                },
                                                                            }
                                                                        }
                                                                        td {
                                                                            input {
                                                                                r#type: "number",
                                                                                placeholder: "Max items",
                                                                                value: "{max_str}",
                                                                                min: "1",
                                                                                oninput: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_max.clone()).or_default();
                                                                                    entry.1 = e.value();
                                                                                },
                                                                            }
                                                                        }
                                                                        td {
                                                                            input {
                                                                                class: "form-input",
                                                                                placeholder: "tag1, tag2",
                                                                                value: "{tags_str}",
                                                                                oninput: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_tags.clone()).or_default();
                                                                                    entry.2 = e.value();
                                                                                },
                                                                            }
                                                                        }
                                                                        td {
                                                                            input {
                                                                                r#type: "checkbox",
                                                                                checked: create_coll,
                                                                                onchange: move |e| {
                                                                                    let mut map = edit_catalog_settings.write();
                                                                                    let entry = map.entry(cid_coll.clone()).or_default();
                                                                                    entry.3 = e.checked();
                                                                                },
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
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| id_to_edit.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: *editing.read() || edit_name_input.read().trim().is_empty(),
                                    onclick: {
                                        let client = app_state.client.clone();
                                        move |_| {
                                            let name = edit_name_input.read().trim().to_string();
                                            if name.is_empty() { return; }
                                            let config: serde_json::Value = serde_json::Value::Object(
                                                edit_form_values.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                            );
                                            // Build resources list from checkboxes.
                                            let resources: Vec<ResourceType> = edit_resources
                                                .read()
                                                .iter()
                                                .filter_map(|s| s.parse::<ResourceType>().ok())
                                                .collect();
                                            let types: Vec<remux_sdks::remux::MediaKind> = edit_types
                                                .read()
                                                .iter()
                                                .filter_map(|s| s.parse::<remux_sdks::remux::MediaKind>().ok())
                                                .collect();
                                            // Build catalog update payload.
                                            let catalog_updates: Vec<UpdateAddonCatalogRequest> = edit_catalog_settings
                                                .read()
                                                .iter()
                                                .map(|(catalog_id, (enabled, max_str, tags_str, create_coll))| {
                                                    let tags: Vec<String> = tags_str
                                                        .split(',')
                                                        .map(|t| t.trim().to_string())
                                                        .filter(|t| !t.is_empty())
                                                        .collect();
                                                    UpdateAddonCatalogRequest {
                                                        catalog_id: catalog_id.clone(),
                                                        enabled: *enabled,
                                                        max_items: max_str.trim().parse::<i64>().ok().filter(|&n| n > 0),
                                                        tags: Some(tags),
                                                        create_collection: Some(*create_coll),
                                                    }
                                                })
                                                .collect();
                                            editing.set(true);
                                            let c = client.clone();
                                            spawn(async move {
                                                let payload = UpdateAddonRequest {
                                                    name: Some(name),
                                                    config: Some(config),
                                                    resources: Some(resources),
                                                    types: Some(types),
                                                    enabled: None,
                                                    priority: None,
                                                };
                                                let addon_res = c.execute(UpdateAddon { id: edit_id, payload }).await;
                                                let cat_res = if !catalog_updates.is_empty() {
                                                    c.execute(UpdateAddonCatalogs { id: edit_id, payload: catalog_updates }).await.err()
                                                } else {
                                                    None
                                                };
                                                match (addon_res, cat_res) {
                                                    (Ok(_), None) => {
                                                        id_to_edit.set(None);
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    }
                                                    (Ok(_), Some(e)) => {
                                                        error.set(Some(format!("Addon saved but catalog update failed: {e}")));
                                                        id_to_edit.set(None);
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    }
                                                    (Err(e), _) => {
                                                        error.set(Some(format!("Failed to update addon: {e}")));
                                                    }
                                                }
                                                editing.set(false);
                                            });
                                        }
                                    },
                                    if *editing.read() { "Saving…" } else { "Save" }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(del_id) = *id_to_delete.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Delete Addon" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.85rem", "Are you sure you want to delete this addon? Catalogs from this addon will be removed on the next import." }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| id_to_delete.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *deleting.read(),
                            style: "background:var(--error);border-color:var(--error)",
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    deleting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(DeleteAddon { id: del_id }).await {
                                            Ok(_) => {
                                                id_to_delete.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to delete addon: {e}")));
                                            }
                                        }
                                        deleting.set(false);
                                    });
                                }
                            },
                            if *deleting.read() { "Deleting…" } else { "Delete" }
                        }
                    }
                }
            }
        }
    }
}

/// Generic form-field renderer driven by an [`AddonOption`] descriptor.
/// Stores the current value back into a shared `values` map keyed by option id.
#[component]
pub(crate) fn AddonOptionField(
    option: AddonOption,
    values: Signal<std::collections::HashMap<String, serde_json::Value>>,
) -> Element {
    let id = option
        .id
        .clone();
    let label = option
        .name
        .clone();
    let desc = option
        .description
        .clone();
    let id_change = id.clone();
    let id_check = id.clone();
    let id_num = id.clone();
    let id_pwd = id.clone();
    let id_text = id.clone();
    let id_select = id.clone();

    let current_str = values
        .read()
        .get(&id)
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| Some(v.to_string()))
        })
        .unwrap_or_default();
    let current_bool = values
        .read()
        .get(&id)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    rsx! {
        div { class: "form-group",
            label { class: "form-label", "{label}" }
            match &option.kind {
                AddonOptionType::Url | AddonOptionType::String => rsx! {
                    input {
                        class: "form-input",
                        r#type: "text",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_change.clone(), serde_json::Value::String(e.value()));
                        },
                    }
                },
                AddonOptionType::Password => rsx! {
                    input {
                        class: "form-input",
                        r#type: "password",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_pwd.clone(), serde_json::Value::String(e.value()));
                        },
                    }
                },
                AddonOptionType::Textarea => rsx! {
                    textarea {
                        class: "form-input",
                        rows: 4,
                        oninput: move |e| {
                            let mut map = values.write();
                            map.insert(id_text.clone(), serde_json::Value::String(e.value()));
                        },
                        "{current_str}"
                    }
                },
                AddonOptionType::Number { .. } => rsx! {
                    input {
                        class: "form-input",
                        r#type: "number",
                        value: "{current_str}",
                        oninput: move |e| {
                            let mut map = values.write();
                            if let Ok(n) = e.value().parse::<i64>() {
                                map.insert(id_num.clone(), serde_json::json!(n));
                            }
                        },
                    }
                },
                AddonOptionType::Boolean => rsx! {
                    label { class: "form-toggle",
                        input {
                            r#type: "checkbox",
                            checked: current_bool,
                            onchange: move |e| {
                                let mut map = values.write();
                                map.insert(id_check.clone(), serde_json::Value::Bool(e.value() == "true"));
                            },
                        }
                        span { "Enabled" }
                    }
                },
                AddonOptionType::Select { options } => rsx! {
                    select {
                        class: "form-input",
                        value: "{current_str}",
                        onchange: move |e| {
                            let mut map = values.write();
                            map.insert(id_select.clone(), serde_json::Value::String(e.value()));
                        },
                        for so in options.iter().cloned() {
                            option { value: "{so.value}", "{so.label}" }
                        }
                    }
                },
                AddonOptionType::MultiSelect { .. } => rsx! {
                    div { class: "field-hint", "(multi-select not yet supported in dashboard)" }
                },
                AddonOptionType::StringList => {
                    let id_list = id.clone();
                    let id_add = id.clone();
                    let current_list: Vec<String> = values
                        .read()
                        .get(&id)
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    rsx! {
                        div { class: "string-list-field",
                            for (i , item) in current_list.iter().enumerate() {
                                {
                                    let item = item.clone();
                                    let id_input = id_list.clone();
                                    let id_remove = id_list.clone();
                                    rsx! {
                                        div {
                                            class: "string-list-row",
                                            style: "display:flex;gap:6px;margin-bottom:4px",
                                            input {
                                                class: "form-input",
                                                r#type: "text",
                                                value: "{item}",
                                                oninput: move |e| {
                                                    let mut map = values.write();
                                                    let arr = map
                                                        .entry(id_input.clone())
                                                        .or_insert_with(|| serde_json::Value::Array(vec![]));
                                                    if let Some(arr) = arr.as_array_mut() {
                                                        if let Some(slot) = arr.get_mut(i) {
                                                            *slot = serde_json::Value::String(e.value());
                                                        }
                                                    }
                                                },
                                            }
                                            button {
                                                class: "btn btn-ghost btn-sm",
                                                r#type: "button",
                                                onclick: move |_| {
                                                    let mut map = values.write();
                                                    if let Some(arr) = map.get_mut(&id_remove) {
                                                        if let Some(arr) = arr.as_array_mut() {
                                                            if i < arr.len() {
                                                                arr.remove(i);
                                                            }
                                                        }
                                                    }
                                                },
                                                "×"
                                            }
                                        }
                                    }
                                }
                            }
                            button {
                                class: "btn btn-ghost btn-sm",
                                r#type: "button",
                                style: "margin-top:2px",
                                onclick: move |_| {
                                    let mut map = values.write();
                                    let arr = map
                                        .entry(id_add.clone())
                                        .or_insert_with(|| serde_json::Value::Array(vec![]));
                                    if let Some(arr) = arr.as_array_mut() {
                                        arr.push(serde_json::Value::String(String::new()));
                                    }
                                },
                                "+ Add"
                            }
                        }
                    }
                }
            }
            if let Some(d) = &desc {
                div { class: "field-hint",
                    for token in d.split_whitespace() {
                        if token.starts_with("https://") || token.starts_with("http://") {
                            a {
                                href: "{token}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                "{token}"
                            }
                            " "
                        } else {
                            "{token} "
                        }
                    }
                }
            }
        }
    }
}
