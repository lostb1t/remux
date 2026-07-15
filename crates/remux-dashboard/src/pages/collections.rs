use crate::{components::*, state::AppState};
use dioxus::prelude::*;
use remux_sdks::remux::{
    BaseItemDto, CollectionFilter, CreateVirtualFolder, CreateVirtualFolderPayload,
    DeleteVirtualFolder, FilterGroup, FilterMatchMode, GetItems, GetItemsQuery,
    ItemSortBy, MediaType, PatchItem, PatchItemPayload, SortOrder,
};

/// Which collection is currently being edited (None = creating new).
#[derive(Clone, Debug)]
pub enum FormMode {
    Create,
    Edit(BaseItemDto),
}

impl PartialEq for FormMode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FormMode::Create, FormMode::Create) => true,
            (FormMode::Edit(a), FormMode::Edit(b)) => a.id == b.id,
            _ => false,
        }
    }
}

#[component]
pub fn CollectionsPage(app_state: AppState) -> Element {
    let mut collections: Signal<Vec<BaseItemDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);
    let mut form_mode: Signal<Option<FormMode>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetItems(GetItemsQuery {
                    include_item_types: Some(vec![MediaType::BoxSet]),
                    include_empty: Some(true),
                    sort_by: Some(vec![ItemSortBy::DisplayOrder]),
                    sort_order: Some(vec![SortOrder::Ascending]),
                    ..Default::default()
                }))
                .await
            {
                Ok(result) => {
                    collections.set(result.items);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to load collections: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Collections" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| form_mode.set(Some(FormMode::Create)),
                    "+ New Collection"
                }
            }
            div { class: "card-body tight",
                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if collections.read().is_empty() {
                    EmptyState { message: "No collections yet" }
                } else {
                    div { class: "data-table-container",
                        div { class: "row-list",
                            {
                                let col_count = collections.read().len();
                                rsx! {
                                for (col_idx, col) in collections.read().clone().into_iter().enumerate() {
                                {
                                    let col_edit = col.clone();
                                    let client_sort = app_state.client.clone();
                                    let col_id_str = col.id.to_string();
                                    let name = col.name.clone().unwrap_or_default();
                                    let col_type_label = match col.collection_type.as_ref() {
                                        Some(ct) => match ct {
                                            remux_sdks::remux::CollectionType::Movies  => "Movies",
                                            remux_sdks::remux::CollectionType::Tvshows => "Shows",
                                            remux_sdks::remux::CollectionType::Mixed   => "Mixed",
                                            remux_sdks::remux::CollectionType::Music   => "Music",
                                            remux_sdks::remux::CollectionType::Boxsets => "Collections",
                                            _ => "Unknown",
                                        },
                                        None => "Unknown",
                                    };
                                    let col_kind_label = match col.remux.as_ref().and_then(|r| r.collection_kind.as_ref()) {
                                        Some(remux_sdks::remux::RemuxCollectionKind::Smart)   => "Smart",
                                        Some(remux_sdks::remux::RemuxCollectionKind::Manual)  => "Manual",
                                        Some(remux_sdks::remux::RemuxCollectionKind::Catalog) => "Catalog",
                                        None => "",
                                    };
                                    rsx! {
                                        div { class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]", key: "{col_id_str}",
                                            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                                div { class: "catalog-name", "{name}" }
                                                div { class: "catalog-meta",
                                                    span { class: "session-client-badge", "{col_type_label}" }
                                                    if !col_kind_label.is_empty() {
                                                        span { class: "session-client-badge", "{col_kind_label}" }
                                                    }
                                                    if col.remux.as_ref().and_then(|r| r.promoted).unwrap_or(false) {
                                                        span { class: "task-badge task-badge-running", "Library" }
                                                    }
                                                }
                                            }
                                            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                                div { class: "addon-card-sort",
                                                    button {
                                                        class: "btn btn-ghost addon-sort-btn",
                                                        disabled: col_idx == 0,
                                                        title: "Move up",
                                                        onclick: {
                                                            let c = client_sort.clone();
                                                            move |_| {
                                                                let current = collections.read().clone();
                                                                if col_idx == 0 { return; }
                                                                let mut new_order = current.clone();
                                                                new_order.swap(col_idx, col_idx - 1);
                                                                let updates: Vec<(String, i64)> = new_order.iter().enumerate()
                                                                    .filter_map(|(i, col)| {
                                                                        let new_so = i as i64 * 10;
                                                                        if col.index_number != Some(new_so) {
                                                                            Some((col.id.to_string(), new_so))
                                                                        } else { None }
                                                                    })
                                                                    .collect();
                                                                let c = c.clone();
                                                                spawn(async move {
                                                                    for (id, so) in updates {
                                                                        let _ = c.execute(PatchItem {
                                                                            item_id: id,
                                                                            payload: PatchItemPayload { sort_order: Some(so), ..Default::default() },
                                                                        }).await;
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
                                                        disabled: col_idx + 1 >= col_count,
                                                        title: "Move down",
                                                        onclick: {
                                                            let c = client_sort.clone();
                                                            move |_| {
                                                                let current = collections.read().clone();
                                                                if col_idx + 1 >= current.len() { return; }
                                                                let mut new_order = current.clone();
                                                                new_order.swap(col_idx, col_idx + 1);
                                                                let updates: Vec<(String, i64)> = new_order.iter().enumerate()
                                                                    .filter_map(|(i, col)| {
                                                                        let new_so = i as i64 * 10;
                                                                        if col.index_number != Some(new_so) {
                                                                            Some((col.id.to_string(), new_so))
                                                                        } else { None }
                                                                    })
                                                                    .collect();
                                                                let c = c.clone();
                                                                spawn(async move {
                                                                    for (id, so) in updates {
                                                                        let _ = c.execute(PatchItem {
                                                                            item_id: id,
                                                                            payload: PatchItemPayload { sort_order: Some(so), ..Default::default() },
                                                                        }).await;
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
                                                    style: "height:30px;font-size:.68rem;padding:0 10px",
                                                    onclick: move |_| form_mode.set(Some(FormMode::Edit(col_edit.clone()))),
                                                    "Edit"
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

        if let Some(mode) = form_mode.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    CollectionForm {
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
pub fn CollectionForm(
    mode: FormMode,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let is_edit = matches!(mode, FormMode::Edit(_));
    let existing: Option<BaseItemDto> = match &mode {
        FormMode::Edit(f) => Some(f.clone()),
        FormMode::Create => None,
    };

    let mut title = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.name
                    .clone()
            })
            .unwrap_or_default()
    });
    let mut promoted = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| r.promoted)
            .unwrap_or(false)
    });
    let mut col_type = use_signal(|| {
        // Prefer the Remux namespace's CollectionMediaKind — it's the canonical source
        // and round-trips correctly for mixed (CollectionType is omitted for mixed).
        if let Some(mk) = existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.collection_media_kind
                    .as_ref()
            })
        {
            return match mk {
                remux_sdks::remux::MediaKind::Movie => "movies".to_string(),
                remux_sdks::remux::MediaKind::Series => "tvshows".to_string(),
                remux_sdks::remux::MediaKind::Mixed => "mixed".to_string(),
                remux_sdks::remux::MediaKind::Track => "music".to_string(),
                remux_sdks::remux::MediaKind::Collection => "collections".to_string(),
                _ => "movies".to_string(),
            };
        }
        // Fallback: infer from CollectionType (covers non-remux legacy items)
        existing
            .as_ref()
            .and_then(|f| {
                f.collection_type
                    .as_ref()
            })
            .map(|ct| match ct {
                remux_sdks::remux::CollectionType::Movies => "movies".to_string(),
                remux_sdks::remux::CollectionType::Tvshows => "tvshows".to_string(),
                remux_sdks::remux::CollectionType::Music => "music".to_string(),
                remux_sdks::remux::CollectionType::Boxsets => "collections".to_string(),
                _ => "movies".to_string(),
            })
            .unwrap_or_else(|| "movies".to_string())
    });
    let mut col_kind = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.collection_kind
                    .as_ref()
            })
            .map(|k| k.to_string())
            .unwrap_or_else(|| "smart".to_string())
    });
    // Smart filter rules
    let sf_match: Signal<FilterMatchMode> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.smart_filter
                    .as_ref()
            })
            .map(|sf| {
                sf.match_mode
                    .clone()
            })
            .unwrap_or(FilterMatchMode::All)
    });
    let sf_groups: Signal<Vec<FilterGroup>> = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.smart_filter
                    .as_ref()
            })
            .map(|sf| {
                sf.groups
                    .clone()
            })
            .unwrap_or_else(|| vec![FilterGroup::default()])
    });
    let tags: Signal<Vec<String>> = use_signal(|| {
        existing
            .as_ref()
            .map(|f| {
                f.tags
                    .clone()
            })
            .unwrap_or_default()
    });
    let mut latest_auto_unplayed = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| r.latest_auto_unplayed)
            .unwrap_or(false)
    });
    let mut latest_sort_digital = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| r.latest_sort_digital)
            .unwrap_or(false)
    });
    // Default sort for catalog / smart collections
    let mut default_sort = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.collection_default_sort
                    .as_ref()
            })
            .and_then(|v| v.first())
            .map(|s| s.to_string())
            .unwrap_or_default()
    });
    let mut default_sort_order = use_signal(|| {
        existing
            .as_ref()
            .and_then(|f| {
                f.remux
                    .as_ref()
            })
            .and_then(|r| {
                r.collection_default_sort_order
                    .as_ref()
            })
            .and_then(|v| v.first())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Descending".to_string())
    });
    let mut saving = use_signal(|| false);
    let mut err = use_signal(|| Option::<String>::None);

    // Image upload state (edit mode only)
    let existing_image_tag = existing
        .as_ref()
        .and_then(|f| {
            f.image_tags
                .as_ref()
        })
        .and_then(|t| {
            t.primary
                .clone()
        });
    let existing_item_id = existing
        .as_ref()
        .map(|f| {
            f.id.to_string()
        });
    let server_base = app_state
        .server
        .manual_address
        .clone();
    let current_image_url = existing_item_id
        .as_ref()
        .zip(existing_image_tag.as_ref())
        .map(|(id, tag)| format!("{server_base}/Items/{id}/Images/Primary?tag={tag}"));
    let mut pending_image_bytes: Signal<Option<Vec<u8>>> = use_signal(|| None);
    let mut pending_image_preview: Signal<Option<String>> = use_signal(|| None);
    let mut has_image = use_signal(|| existing_image_tag.is_some());
    let client_for_delete = app_state
        .client
        .clone();
    let app_state_delete = app_state.clone();
    let delete_name = existing
        .as_ref()
        .and_then(|f| {
            f.name
                .clone()
        })
        .unwrap_or_default();

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let item_id = existing
            .as_ref()
            .map(|f| {
                f.id.to_string()
            });
        let name = title
            .peek()
            .clone();
        let ct = col_type
            .peek()
            .clone();
        let ck = col_kind
            .peek()
            .clone();
        let prm = *promoted.peek();
        let auto_unplayed = *latest_auto_unplayed.peek();
        let sort_digital = *latest_sort_digital.peek();
        let current_tags = tags
            .peek()
            .clone();
        let smart_filter_payload = if ck == "smart" || ck == "catalog" {
            Some(CollectionFilter {
                match_mode: sf_match
                    .peek()
                    .clone(),
                groups: sf_groups
                    .peek()
                    .clone(),
            })
        } else {
            None
        };
        let ds = default_sort
            .peek()
            .clone();
        let dso = default_sort_order
            .peek()
            .clone();
        let default_sort_payload: Option<Vec<ItemSortBy>> = if ds.is_empty() {
            Some(vec![])
        } else {
            ds.parse::<ItemSortBy>()
                .ok()
                .map(|s| vec![s])
        };
        let default_sort_order_payload: Option<Vec<SortOrder>> = if ds.is_empty() {
            Some(vec![])
        } else {
            default_sort_payload
                .as_ref()
                .map(|_| {
                    vec![dso
                        .parse::<SortOrder>()
                        .unwrap_or(SortOrder::Ascending)]
                })
        };
        saving.set(true);
        err.set(None);
        let pending_bytes = pending_image_bytes
            .peek()
            .clone();
        spawn(async move {
            let result = if let Some(id) = item_id {
                // Edit existing collection
                let patch = client
                    .execute(PatchItem {
                        item_id: id.clone(),
                        payload: PatchItemPayload {
                            name: Some(name),
                            collection_type: Some(ct),
                            collection_kind: Some(ck),
                            smart_filter: smart_filter_payload,
                            promoted: Some(prm),
                            tags: Some(current_tags),
                            sort_order: None,
                            latest_auto_unplayed: Some(auto_unplayed),
                            latest_sort_digital: Some(sort_digital),
                            collection_default_sort: default_sort_payload,
                            collection_default_sort_order: default_sort_order_payload,
                        },
                    })
                    .await;
                if patch.is_ok() {
                    if let Some(bytes) = pending_bytes {
                        let ct = crate::state::detect_image_content_type(&bytes);
                        let _ = client
                            .execute(remux_sdks::remux::UploadItemImage {
                                item_id: id,
                                image_type: "Primary".to_string(),
                                bytes,
                                content_type: ct,
                            })
                            .await;
                    }
                }
                patch
            } else {
                // Create new collection, then patch extra fields the create endpoint doesn't accept
                let info = match client
                    .execute(CreateVirtualFolder {
                        payload: CreateVirtualFolderPayload {
                            name,
                            collection_type: Some(ct),
                            collection_kind: Some(ck),
                            promoted: Some(prm),
                            sort_order: None,
                        },
                    })
                    .await
                {
                    Ok(info) => info,
                    Err(e) => return err.set(Some(e.user_message())),
                };
                let Some(new_id) = info.item_id else {
                    return on_done.call(());
                };
                let patch = client
                    .execute(PatchItem {
                        item_id: new_id.clone(),
                        payload: PatchItemPayload {
                            name: None,
                            collection_type: None,
                            collection_kind: None,
                            smart_filter: smart_filter_payload,
                            promoted: None,
                            tags: Some(current_tags),
                            sort_order: None,
                            latest_auto_unplayed: Some(auto_unplayed),
                            latest_sort_digital: Some(sort_digital),
                            collection_default_sort: default_sort_payload,
                            collection_default_sort_order: default_sort_order_payload,
                        },
                    })
                    .await;
                if patch.is_ok() {
                    if let Some(bytes) = pending_bytes {
                        let ct = crate::state::detect_image_content_type(&bytes);
                        let _ = client
                            .execute(remux_sdks::remux::UploadItemImage {
                                item_id: new_id,
                                image_type: "Primary".to_string(),
                                bytes,
                                content_type: ct,
                            })
                            .await;
                    }
                }
                patch
            };
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
            if is_edit { "Edit Collection" } else { "New Collection" }
        }

        form {
            onsubmit: on_submit,
            style: "display:flex;flex-direction:column;gap:14px",

            div { class: "field",
                label { class: "field-label", r#for: "col-title", "Title" }
                input {
                    id: "col-title",
                    r#type: "text",
                    class: "field-input",
                    required: true,
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                }
            }

            div { class: "field",
                label { class: "field-label", r#for: "col-type", "Media Kind" }
                select {
                    id: "col-type",
                    class: "select-input",
                    value: "{col_type}",
                    onchange: move |e| col_type.set(e.value()),
                    option { value: "movies",      "Movies"      }
                    option { value: "tvshows",     "TV Shows"    }
                    option { value: "mixed",       "Mixed (Movies & Shows)" }
                    option { value: "music",       "Music"       }
                    option { value: "collections", "Collections" }
                }
            }

            if col_type.read().as_str() != "collections" {
                div { class: "field",
                    label { class: "field-label", r#for: "col-kind", "Collection Kind" }
                    select {
                        id: "col-kind",
                        class: "select-input",
                        value: "{col_kind}",
                        disabled: is_edit,
                        onchange: move |e| col_kind.set(e.value()),
                        option { value: "smart",  "Smart"  }
                        option { value: "manual", "Manual" }
                    }
                }
            }

            div { class: "field",
                label { class: "field-label", "Tags" }
                TagChipInput { tags }
            }

            if is_edit {
                div { class: "field",
                    label { class: "field-label", "Image" }
                    div { style: "display:flex;flex-direction:column;gap:8px",
                        // Preview: local pick takes priority over server image
                        if let Some(preview) = pending_image_preview.read().as_ref() {
                            img {
                                src: "{preview}",
                                style: "width:100%;max-height:180px;object-fit:cover;border-radius:6px;border:1px solid var(--border)",
                            }
                        } else if let Some(url) = &current_image_url {
                            if *has_image.read() {
                                img {
                                    src: "{url}",
                                    style: "width:100%;max-height:180px;object-fit:cover;border-radius:6px;border:1px solid var(--border)",
                                    onerror: move |_| has_image.set(false),
                                }
                            }
                        }
                        div { style: "display:flex;gap:8px;align-items:center",
                            label {
                                class: "btn btn-ghost",
                                style: "height:30px;font-size:.68rem;padding:0 10px;cursor:pointer",
                                input {
                                    r#type: "file",
                                    accept: "image/*",
                                    style: "display:none",
                                    onchange: move |e| {
                                        spawn(async move {
                                            let files_data = e.files();
                                            if let Some(file_data) = files_data.first() {
                                                if let Ok(raw) = file_data.read_bytes().await {
                                                    let bytes: Vec<u8> = raw.to_vec();
                                                    let ct = crate::state::detect_image_content_type(&bytes);
                                                    let b64 = base64::Engine::encode(
                                                        &base64::engine::general_purpose::STANDARD,
                                                        &bytes
                                                    );
                                                    let data_url = format!("data:{ct};base64,{b64}");
                                                    pending_image_preview.set(Some(data_url));
                                                    pending_image_bytes.set(Some(bytes));
                                                    has_image.set(true);
                                                }
                                            }
                                        });
                                    },
                                }
                                "Choose image"
                            }
                            if *has_image.read() {
                                button {
                                    r#type: "button",
                                    class: "btn btn-ghost",
                                    style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                    onclick: {
                                        let item_id = existing_item_id.clone();
                                        let client = client_for_delete.clone();
                                        move |_| {
                                            let item_id = item_id.clone();
                                            let client = client.clone();
                                            spawn(async move {
                                                if let Some(id) = item_id {
                                                    let _ = client.execute(remux_sdks::remux::DeleteItemImage {
                                                        item_id: id,
                                                        image_type: "Primary".to_string(),
                                                    }).await;
                                                }
                                                pending_image_bytes.set(None);
                                                pending_image_preview.set(None);
                                                has_image.set(false);
                                            });
                                        }
                                    },
                                    "Remove image"
                                }
                            }
                        }
                    }
                }
            }

            ToggleRow {
                label: "Promote to Library",
                checked: *promoted.read(),
                on_change: move |v| promoted.set(v),
            }

            ToggleRow {
                label: "Latest: Unplayed Only",
                checked: *latest_auto_unplayed.read(),
                on_change: move |v| latest_auto_unplayed.set(v),
            }

            ToggleRow {
                label: "Latest: Sort by Digital Release",
                checked: *latest_sort_digital.read(),
                on_change: move |v| latest_sort_digital.set(v),
            }

            if (col_kind.read().as_str() == "smart" || col_kind.read().as_str() == "catalog") && col_type.read().as_str() != "collections" {
                FilterRuleEditor { match_mode: sf_match, groups: sf_groups }

                div { class: "field",
                    label { class: "field-label", "Default Sort Override" }
                    p { class: "field-hint", "Overrides the sort order when the client sends no preference or its default (Sort Name). Note: the client UI may still show its own sort label." }
                    div { style: "display:flex;gap:8px",
                        select {
                            class: "select-input",
                            style: "flex:1;min-width:0",
                            value: "{default_sort}",
                            onchange: move |e| default_sort.set(e.value()),
                            option { value: "", selected: default_sort.read().is_empty(), "— None —" }
                            if sf_groups.read().iter().flat_map(|g| g.rules.iter()).any(|r| matches!(r, remux_sdks::remux::FilterRule::Catalog { .. })) {
                                option { value: "CatalogOrder", selected: *default_sort.read() == "CatalogOrder", "Catalog Order" }
                            }
                            option { value: "SortName",           selected: *default_sort.read() == "SortName",           "Name" }
                            option { value: "PremiereDate",       selected: *default_sort.read() == "PremiereDate",       "Release Date" }
                            option { value: "DigitalReleaseDate", selected: *default_sort.read() == "DigitalReleaseDate", "Digital Release Date" }
                            option { value: "DateCreated",        selected: *default_sort.read() == "DateCreated",        "Date Added" }
                            option { value: "CommunityRating",    selected: *default_sort.read() == "CommunityRating",    "Community Rating" }
                            option { value: "PopularityDay",      selected: *default_sort.read() == "PopularityDay",      "Popularity (Today)" }
                            option { value: "PopularityWeek",     selected: *default_sort.read() == "PopularityWeek",     "Popularity (This Week)" }
                            option { value: "PopularityMonth",    selected: *default_sort.read() == "PopularityMonth",    "Popularity (This Month)" }
                            option { value: "PopularityAllTime",  selected: *default_sort.read() == "PopularityAllTime",  "Popularity (All Time)" }
                            option { value: "TrendingWeek",       selected: *default_sort.read() == "TrendingWeek",       "Trending (7 days)" }
                            option { value: "TrendingMonth",      selected: *default_sort.read() == "TrendingMonth",      "Trending (30 days)" }
                            option { value: "Random",             selected: *default_sort.read() == "Random",             "Random" }
                        }
                        if !default_sort.read().is_empty() && *default_sort.read() != "Random" {
                            select {
                                class: "select-input",
                                style: "flex:0 0 auto;width:auto",
                                value: "{default_sort_order}",
                                onchange: move |e| default_sort_order.set(e.value()),
                                option { value: "Ascending",  selected: *default_sort_order.read() == "Ascending",  "Asc" }
                                option { value: "Descending", selected: *default_sort_order.read() == "Descending", "Desc" }
                            }
                        }
                    }
                }
            }

            if let Some(e) = err.read().as_ref() {
                ErrorAlert { message: e.clone() }
            }

            FormActions {
                if is_edit {
                    button {
                        r#type: "button",
                        class: "btn btn-ghost",
                        style: "color:var(--error);border-color:var(--error);margin-right:auto",
                        onclick: {
                            let client = app_state_delete.client.clone();
                            let name = delete_name.clone();
                            move |_| {
                                let client = client.clone();
                                let name = name.clone();
                                spawn(async move {
                                    let _ = client.execute(DeleteVirtualFolder { name }).await;
                                    on_done.call(());
                                });
                            }
                        },
                        "Delete"
                    }
                }
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
