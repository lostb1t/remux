use crate::{
    components::{
        Button, ButtonVariant, EmptyState, ErrorAlert, FormActions, LoadingText, Modal,
        TagChipInput,
    },
    router::Route,
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    AddonCatalogDto, AddonDto, CollectionFilter, CreateVirtualFolder,
    CreateVirtualFolderPayload, FilterGroup, FilterMatchMode, FilterRule,
    GetAddonCatalogs, GetItems, GetItemsQuery, ListAddons, MediaKind, MediaType,
    PatchItem, PatchItemPayload, SetOp,
};
use std::collections::HashSet;
use uuid::Uuid;

/// Maps a catalog's media kind to the `collection_type` string the collection
/// creation endpoint expects. Falls back to "movies" when the addon hasn't told
/// us the kind (mirrors the default used by the manual "New Collection" form).
fn collection_type_for(kind: Option<&MediaKind>) -> &'static str {
    match kind {
        Some(MediaKind::Series) => "tvshows",
        Some(MediaKind::Mixed) => "mixed",
        Some(MediaKind::Track) => "music",
        Some(MediaKind::Collection) => "collections",
        _ => "movies",
    }
}

/// Builds the `"{addon_uuid}:{local_catalog_id}"` provenance string stored on
/// `collection_source`, stripping the `addon:{addon_uuid}:` prefix that
/// `catalog_id` carries (see `remux_server::addons::make_media_id`).
fn collection_source_for(addon_id: Uuid, catalog_id: &str) -> String {
    let prefix = format!("addon:{addon_id}:");
    let local_id = catalog_id
        .strip_prefix(&prefix)
        .unwrap_or(catalog_id);
    format!("{addon_id}:{local_id}")
}

/// Picks a `collection_type` for a merged collection spanning several
/// catalogs: their shared kind if they all agree, otherwise "mixed".
fn merged_collection_type(cats: &[AddonCatalogDto]) -> &'static str {
    let mut kinds = cats
        .iter()
        .map(|c| {
            c.collection_media_kind
                .clone()
        });
    let first = kinds
        .next()
        .flatten();
    if kinds.all(|k| k == first) {
        collection_type_for(first.as_ref())
    } else {
        "mixed"
    }
}

#[component]
pub fn AddonCatalogsPage(app_state: AppState, addon_id: Uuid) -> Element {
    let mut addon: Signal<Option<AddonDto>> = use_signal(|| None);
    let mut catalogs: Signal<Vec<AddonCatalogDto>> = use_signal(Vec::new);
    let mut existing_sources: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let mut selected: Signal<HashSet<String>> = use_signal(HashSet::new);
    let mut search = use_signal(String::new);
    let mut merge_mode = use_signal(|| false);
    let mut merge_name = use_signal(String::new);
    // "" means "auto-detect from the catalog(s)" — matches the existing per-catalog
    // and merged-kind inference already used when this is left unset.
    let mut kind_override = use_signal(String::new);
    let mut tags: Signal<Vec<String>> = use_signal(Vec::new);
    let mut show_confirm = use_signal(|| false);
    let mut creating = use_signal(|| false);
    let mut create_progress: Signal<Option<(usize, usize)>> = use_signal(|| None);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);
    let mut created_count = use_signal(|| 0_usize);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            let addons_res = client
                .execute(ListAddons)
                .await;
            let catalogs_res = client
                .execute(GetAddonCatalogs { id: addon_id })
                .await;
            // Existing collections tell us which catalogs have already been
            // imported (via `remux.collectionSource`), so we can grey those out
            // instead of letting the user create duplicates.
            let items_res = client
                .execute(GetItems(GetItemsQuery {
                    include_item_types: Some(vec![MediaType::BoxSet]),
                    ..Default::default()
                }))
                .await;
            match (addons_res, catalogs_res) {
                (Ok(addons), Ok(cats)) => {
                    addon.set(
                        addons
                            .into_iter()
                            .find(|a| a.id == addon_id),
                    );
                    catalogs.set(cats);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load catalogs: {e}")));
                }
            }
            if let Ok(result) = items_res {
                let sources: HashSet<String> = result
                    .items
                    .into_iter()
                    .filter_map(|item| {
                        item.remux
                            .and_then(|r| r.collection_source)
                    })
                    .collect();
                existing_sources.set(sources);
            }
            loading.set(false);
        });
    });

    // Only catalogs with a resolved collection id can be turned into a
    // collection filter — addons that haven't finished indexing won't have one yet.
    let eligible: Vec<AddonCatalogDto> = catalogs
        .read()
        .iter()
        .filter(|c| {
            c.collection_id
                .is_some()
        })
        .cloned()
        .collect();
    let skipped_count = catalogs
        .read()
        .len()
        .saturating_sub(eligible.len());

    // Catalogs that already have a collection pointing at them (matched via
    // collection_source) can't be selected again in "one collection per catalog"
    // mode — no point offering duplicates. In merge mode they're fine to include,
    // since the result is a different, new collection either way.
    let is_imported = |cat: &AddonCatalogDto| -> bool {
        existing_sources
            .read()
            .contains(&collection_source_for(addon_id, &cat.catalog_id))
    };
    let imported_count = eligible
        .iter()
        .filter(|c| is_imported(c))
        .count();
    let selectable: Vec<AddonCatalogDto> = if *merge_mode.read() {
        eligible.clone()
    } else {
        eligible
            .iter()
            .filter(|c| !is_imported(c))
            .cloned()
            .collect()
    };

    let search_lower = search
        .read()
        .trim()
        .to_lowercase();
    let matches_search = |cat: &AddonCatalogDto| -> bool {
        search_lower.is_empty()
            || cat
                .name
                .to_lowercase()
                .contains(&search_lower)
    };
    let displayed: Vec<AddonCatalogDto> = eligible
        .iter()
        .filter(|c| matches_search(c))
        .cloned()
        .collect();
    let selectable_visible: Vec<AddonCatalogDto> = selectable
        .iter()
        .filter(|c| matches_search(c))
        .cloned()
        .collect();

    let all_selected = !selectable_visible.is_empty()
        && selectable_visible
            .iter()
            .all(|c| {
                selected
                    .read()
                    .contains(&c.catalog_id)
            });

    let addon_name = addon
        .read()
        .as_ref()
        .map(|a| {
            a.name
                .clone()
        })
        .unwrap_or_else(|| "Addon".to_string());

    let client_for_separate = app_state
        .client
        .clone();
    let on_create_separate = move |_| {
        show_confirm.set(false);
        let client = client_for_separate.clone();
        let kind_choice = kind_override
            .read()
            .clone();
        let tags_choice = tags
            .read()
            .clone();
        let to_create: Vec<AddonCatalogDto> = catalogs
            .read()
            .iter()
            .filter(|c| {
                selected
                    .read()
                    .contains(&c.catalog_id)
            })
            .cloned()
            .collect();
        if to_create.is_empty() {
            return;
        }
        creating.set(true);
        create_error.set(None);
        created_count.set(0);
        let total = to_create.len();
        create_progress.set(Some((0, total)));
        spawn(async move {
            let mut failures: Vec<String> = vec![];
            for (i, cat) in to_create
                .into_iter()
                .enumerate()
            {
                create_progress.set(Some((i + 1, total)));
                let Some(collection_id) = cat.collection_id else {
                    continue;
                };
                let collection_type = if kind_choice.is_empty() {
                    collection_type_for(
                        cat.collection_media_kind
                            .as_ref(),
                    )
                } else {
                    kind_choice.as_str()
                };

                let info = match client
                    .execute(CreateVirtualFolder {
                        payload: CreateVirtualFolderPayload {
                            name: cat
                                .name
                                .clone(),
                            collection_type: Some(collection_type.to_string()),
                            collection_kind: Some("catalog".to_string()),
                            promoted: Some(false),
                            sort_order: None,
                        },
                    })
                    .await
                {
                    Ok(info) => info,
                    Err(e) => {
                        failures.push(format!("{}: {}", cat.name, e.user_message()));
                        continue;
                    }
                };
                let Some(new_id) = info.item_id else {
                    failures.push(format!("{}: no item id returned", cat.name));
                    continue;
                };

                let smart_filter = CollectionFilter {
                    match_mode: FilterMatchMode::All,
                    groups: vec![FilterGroup {
                        match_mode: FilterMatchMode::All,
                        rules: vec![FilterRule::Catalog {
                            op: SetOp::Is,
                            catalog_ids: vec![collection_id],
                        }],
                    }],
                };
                let patch = client
                    .execute(PatchItem {
                        item_id: new_id,
                        payload: PatchItemPayload {
                            smart_filter: Some(smart_filter),
                            collection_source: Some(collection_source_for(
                                addon_id,
                                &cat.catalog_id,
                            )),
                            tags: if tags_choice.is_empty() {
                                None
                            } else {
                                Some(tags_choice.clone())
                            },
                            ..Default::default()
                        },
                    })
                    .await;
                match patch {
                    Ok(_) => {
                        let v = *created_count.peek() + 1;
                        created_count.set(v);
                    }
                    Err(e) => {
                        failures.push(format!("{}: {}", cat.name, e.user_message()))
                    }
                }
            }
            creating.set(false);
            create_progress.set(None);
            if !failures.is_empty() {
                create_error.set(Some(format!(
                    "{} of {} failed: {}",
                    failures.len(),
                    total,
                    failures.join("; ")
                )));
            }
            selected.set(HashSet::new());
            tags.set(Vec::new());
            kind_override.set(String::new());
        });
    };

    let on_create_merge = move |_| {
        show_confirm.set(false);
        let name = merge_name
            .read()
            .trim()
            .to_string();
        if name.is_empty() {
            create_error
                .set(Some("Give the merged collection a name first.".to_string()));
            return;
        }
        let client = app_state
            .client
            .clone();
        let kind_choice = kind_override
            .read()
            .clone();
        let tags_choice = tags
            .read()
            .clone();
        let to_create: Vec<AddonCatalogDto> = catalogs
            .read()
            .iter()
            .filter(|c| {
                selected
                    .read()
                    .contains(&c.catalog_id)
            })
            .cloned()
            .collect();
        if to_create.is_empty() {
            return;
        }
        creating.set(true);
        create_error.set(None);
        created_count.set(0);
        create_progress.set(None);
        spawn(async move {
            let collection_ids: Vec<Uuid> = to_create
                .iter()
                .filter_map(|c| c.collection_id)
                .collect();
            let collection_type = if kind_choice.is_empty() {
                merged_collection_type(&to_create).to_string()
            } else {
                kind_choice.clone()
            };

            let result: Result<(), String> = async {
                let info = client
                    .execute(CreateVirtualFolder {
                        payload: CreateVirtualFolderPayload {
                            name: name.clone(),
                            collection_type: Some(collection_type.to_string()),
                            collection_kind: Some("smart".to_string()),
                            promoted: Some(false),
                            sort_order: None,
                        },
                    })
                    .await
                    .map_err(|e| e.user_message())?;
                let new_id = info
                    .item_id
                    .ok_or_else(|| "no item id returned".to_string())?;

                let smart_filter = CollectionFilter {
                    match_mode: FilterMatchMode::All,
                    groups: vec![FilterGroup {
                        match_mode: FilterMatchMode::All,
                        rules: vec![FilterRule::Catalog {
                            op: SetOp::In,
                            catalog_ids: collection_ids,
                        }],
                    }],
                };
                client
                    .execute(PatchItem {
                        item_id: new_id,
                        payload: PatchItemPayload {
                            smart_filter: Some(smart_filter),
                            tags: if tags_choice.is_empty() {
                                None
                            } else {
                                Some(tags_choice.clone())
                            },
                            ..Default::default()
                        },
                    })
                    .await
                    .map_err(|e| e.user_message())?;
                Ok(())
            }
            .await;

            creating.set(false);
            match result {
                Ok(()) => {
                    created_count.set(1);
                    selected.set(HashSet::new());
                    merge_name.set(String::new());
                    tags.set(Vec::new());
                    kind_override.set(String::new());
                }
                Err(e) => create_error.set(Some(format!("{name}: {e}"))),
            }
        });
    };

    rsx! {
        div { class: "card",
            div { class: "card-header", style: "flex-wrap:wrap;row-gap:10px",
                div { style: "display:flex;align-items:center;gap:10px;min-width:0",
                    button {
                        class: "btn btn-ghost",
                        style: "height:28px;font-size:.68rem;padding:0 10px",
                        onclick: move |_| { navigator().push(Route::AddonsRoute); },
                        "← Addons"
                    }
                    span { class: "card-title", "{addon_name} — Catalogs" }
                    if !eligible.is_empty() {
                        div { class: "tab-group",
                            button {
                                r#type: "button",
                                class: if !*merge_mode.read() { "tab-btn active" } else { "tab-btn" },
                                disabled: *creating.read(),
                                onclick: move |_| {
                                    merge_mode.set(false);
                                    selected.set(HashSet::new());
                                },
                                "Separate collections"
                            }
                            button {
                                r#type: "button",
                                class: if *merge_mode.read() { "tab-btn active" } else { "tab-btn" },
                                disabled: *creating.read(),
                                onclick: move |_| {
                                    merge_mode.set(true);
                                    selected.set(HashSet::new());
                                },
                                "Merge into one"
                            }
                        }
                    }
                }
                if !eligible.is_empty() {
                    div { style: "display:flex;gap:8px;align-items:center;flex-wrap:wrap",
                        input {
                            r#type: "text",
                            class: "field-input",
                            style: "height:32px;width:200px",
                            placeholder: "Search catalogs…",
                            value: "{search}",
                            oninput: move |e| search.set(e.value()),
                        }
                        span { class: "field-hint", "{selected.read().len()} of {selectable_visible.len()} selected" }
                        button {
                            class: "btn btn-ghost",
                            style: "height:32px;font-size:.68rem",
                            disabled: *creating.read() || selectable_visible.is_empty(),
                            onclick: move |_| {
                                if all_selected {
                                    for c in selectable_visible.iter() {
                                        selected.write().remove(&c.catalog_id);
                                    }
                                } else {
                                    let mut set = selected.write();
                                    for c in selectable_visible.iter() {
                                        set.insert(c.catalog_id.clone());
                                    }
                                }
                            },
                            if all_selected { "Select None" } else { "Select All" }
                        }
                    }
                }
            }
            if !eligible.is_empty() {
                div {
                    style: "display:flex;gap:14px;align-items:flex-end;flex-wrap:wrap;padding:14px 20px;border-bottom:1px solid var(--border)",
                    div { class: "field", style: "margin:0;min-width:170px",
                        label { class: "field-label", "Media Kind" }
                        select {
                            class: "select-input",
                            value: "{kind_override}",
                            disabled: *creating.read(),
                            onchange: move |e| kind_override.set(e.value()),
                            option { value: "", "Auto-detect" }
                            option { value: "movies", "Movies" }
                            option { value: "tvshows", "TV Shows" }
                            option { value: "mixed", "Mixed (Movies & Shows)" }
                            option { value: "music", "Music" }
                            option { value: "collections", "Collections" }
                        }
                    }
                    div { class: "field", style: "margin:0;min-width:220px;flex:1",
                        label { class: "field-label", "Tags" }
                        TagChipInput { tags }
                    }
                    if *merge_mode.read() {
                        div { class: "field", style: "margin:0;min-width:220px",
                            label { class: "field-label", "Collection Name" }
                            input {
                                r#type: "text",
                                class: "field-input",
                                placeholder: "Collection name…",
                                value: "{merge_name}",
                                disabled: *creating.read(),
                                oninput: move |e| merge_name.set(e.value()),
                            }
                        }
                        button {
                            class: "btn btn-primary",
                            style: "height:36px;font-size:.7rem",
                            disabled: *creating.read()
                                || selected.read().is_empty()
                                || merge_name.read().trim().is_empty(),
                            onclick: move |_| show_confirm.set(true),
                            if *creating.read() { "Creating…" } else { "+ Create Merged Collection" }
                        }
                    } else {
                        button {
                            class: "btn btn-primary",
                            style: "height:36px;font-size:.7rem",
                            disabled: *creating.read() || selected.read().is_empty(),
                            onclick: move |_| show_confirm.set(true),
                            if let Some((done, total)) = *create_progress.read() {
                                "Creating {done}/{total}…"
                            } else {
                                "+ Create Collections"
                            }
                        }
                    }
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if catalogs.read().is_empty() {
                    EmptyState { message: "This addon doesn't expose any catalogs." }
                } else {
                    div { style: "padding:16px 16px 0",
                        p { class: "field-hint",
                            if *merge_mode.read() {
                                "Select the catalogs to combine, name the collection, then create it. "
                                "It'll include anything in any of the selected catalogs."
                            } else {
                                "Select the catalogs you want as collections, then create them all at once. "
                                "Each one becomes its own collection filtered to that catalog."
                            }
                        }
                        if skipped_count > 0 {
                            p { class: "field-hint", style: "color:var(--error)",
                                "{skipped_count} catalog(s) haven't finished indexing yet and can't be added right now."
                            }
                        }
                    }
                    if *created_count.read() > 0 && !*creating.read() {
                        div { style: "padding:0 16px",
                            span { class: "field-hint", style: "color:var(--success, #2e7d32)",
                                "Created {*created_count.read()} collection(s). "
                                a { onclick: move |_| { navigator().push(Route::LibraryRoute); }, style: "cursor:pointer;text-decoration:underline", "View in Library →" }
                            }
                        }
                    }
                    if let Some(e) = create_error.read().as_ref() {
                        div { style: "padding:8px 16px 0", ErrorAlert { message: e.clone() } }
                    }
                    if imported_count > 0 && !*merge_mode.read() {
                        div { style: "padding:0 16px",
                            p { class: "field-hint",
                                "{imported_count} catalog(s) already have a collection and are shown greyed out."
                            }
                        }
                    }
                    if !displayed.is_empty() {
                        div { class: "addon-kind-list", style: "padding:16px",
                            for cat in displayed.iter().cloned() {
                            {
                                let cid = cat.catalog_id.clone();
                                let cid_click = cid.clone();
                                let imported = is_imported(&cat);
                                let blocked = imported && !*merge_mode.read();
                                let is_selected = !blocked && selected.read().contains(&cid);
                                let kind_label = match cat.collection_media_kind {
                                    Some(MediaKind::Series) => "Shows",
                                    Some(MediaKind::Mixed) => "Mixed",
                                    Some(MediaKind::Track) => "Music",
                                    Some(MediaKind::Collection) => "Collections",
                                    _ => "Movies",
                                };
                                let card_class = if is_selected {
                                    "addon-kind-card addon-kind-card--selected"
                                } else {
                                    "addon-kind-card"
                                };
                                let card_style = if blocked {
                                    "opacity:.5;cursor:default"
                                } else if *creating.read() {
                                    "pointer-events:none;opacity:.6"
                                } else {
                                    ""
                                };
                                rsx! {
                                    div {
                                        key: "{cid}",
                                        class: card_class,
                                        style: card_style,
                                        onclick: move |_| {
                                            if blocked {
                                                return;
                                            }
                                            let mut set = selected.write();
                                            if set.contains(&cid_click) {
                                                set.remove(&cid_click);
                                            } else {
                                                set.insert(cid_click.clone());
                                            }
                                        },
                                        div { class: "addon-kind-card-name", "{cat.name}" }
                                        div { class: "addon-kind-card-badges",
                                            span { class: "addon-kind-type", "{kind_label}" }
                                            span { class: "addon-kind-badge",
                                                match cat.item_count {
                                                    Some(n) => format!("{n} items"),
                                                    None => "not indexed yet".to_string(),
                                                }
                                            }
                                            if imported {
                                                span { class: "addon-kind-badge", "Already added" }
                                            }
                                            if !cat.enabled {
                                                span { class: "addon-kind-badge", "Disabled in addon" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        }
                    } else {
                        div { style: "padding:16px",
                            p { class: "field-hint", "No catalogs match \"{search}\"." }
                        }
                    }
                }
            }
        }
        if *show_confirm.read() {
            {
                let selected_list: Vec<AddonCatalogDto> = catalogs
                    .read()
                    .iter()
                    .filter(|c| selected.read().contains(&c.catalog_id))
                    .cloned()
                    .collect();
                let kind_label = match kind_override.read().as_str() {
                    "movies" => "Movies",
                    "tvshows" => "TV Shows",
                    "mixed" => "Mixed (Movies & Shows)",
                    "music" => "Music",
                    "collections" => "Collections",
                    _ => "Auto-detect (per catalog)",
                };
                let tags_display = tags
                    .read()
                    .join(", ");
                rsx! {
                    Modal {
                        on_close: move |_| show_confirm.set(false),
                        p { class: "modal-title",
                            if *merge_mode.read() { "Create merged collection?" } else { "Create {selected_list.len()} collection(s)?" }
                        }
                        div { style: "display:flex;flex-direction:column;gap:6px;margin:12px 0 16px",
                            if *merge_mode.read() {
                                p { class: "field-hint", "Name: {merge_name}" }
                            }
                            p { class: "field-hint", "Media Kind: {kind_label}" }
                            if !tags_display.is_empty() {
                                p { class: "field-hint", "Tags: {tags_display}" }
                            }
                            p { class: "field-hint",
                                if *merge_mode.read() {
                                    "This combines the following catalogs into one collection:"
                                } else {
                                    "This creates one collection per catalog below:"
                                }
                            }
                            ul { style: "margin:0;padding-left:18px;max-height:220px;overflow-y:auto",
                                for cat in selected_list.iter() {
                                    li { key: "{cat.catalog_id}", class: "field-hint", "{cat.name}" }
                                }
                            }
                        }
                        FormActions {
                            Button {
                                variant: ButtonVariant::Ghost,
                                onclick: move |_| show_confirm.set(false),
                                "Cancel"
                            }
                            if *merge_mode.read() {
                                Button {
                                    variant: ButtonVariant::Primary,
                                    onclick: on_create_merge,
                                    "Confirm & Merge"
                                }
                            } else {
                                Button {
                                    variant: ButtonVariant::Primary,
                                    onclick: on_create_separate,
                                    "Confirm & Create"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
