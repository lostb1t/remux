use crate::{
    components::{EmptyState, FormGroup, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CreateStreamGroup, CreateStreamGroupRequest, DeleteStreamGroup, FilterMatchMode,
    GetStreamGroupPreview, GetSystemConfiguration, ListStreamGroups,
    ServerConfiguration, SetOp, StreamCodec, StreamFilter, StreamGroupDto,
    StreamGroupPreviewDto, StreamQuality, StreamResolution, StreamRule,
    UpdateStreamGroup, UpdateStreamGroupRequest, UpdateSystemConfiguration,
};
use uuid::Uuid;

#[component]
pub(crate) fn StreamRuleRow(
    idx: usize,
    rule: StreamRule,
    rules: Signal<Vec<StreamRule>>,
) -> Element {
    let field_val = match &rule {
        StreamRule::Resolution { .. } => "resolution",
        StreamRule::Quality { .. } => "quality",
        StreamRule::Codec { .. } => "codec",
    };
    let op_not_in = match &rule {
        StreamRule::Resolution { op, .. }
        | StreamRule::Quality { op, .. }
        | StreamRule::Codec { op, .. } => matches!(op, SetOp::NotIn),
    };

    rsx! {
        div { style: "display:flex;align-items:flex-start;gap:6px",
            // Field selector
            select {
                class: "select-input",
                style: "flex:1.2",
                value: "{field_val}",
                onchange: move |e| {
                    if let Some(r) = rules.write().get_mut(idx) {
                        *r = match e.value().as_str() {
                            "quality" => StreamRule::Quality { op: SetOp::In, values: vec![] },
                            "codec"  => StreamRule::Codec  { op: SetOp::In, values: vec![] },
                            _        => StreamRule::Resolution { op: SetOp::In, values: vec![] },
                        };
                    }
                },
                option { value: "resolution", selected: field_val == "resolution", "Resolution" }
                option { value: "quality",     selected: field_val == "quality",     "Quality" }
                option { value: "codec",      selected: field_val == "codec",      "Codec" }
            }
            // Operator selector
            select {
                class: "select-input",
                style: "flex:1",
                onchange: move |e| {
                    let new_op = if e.value() == "not_in" { SetOp::NotIn } else { SetOp::In };
                    if let Some(r) = rules.write().get_mut(idx) {
                        *r = match r.clone() {
                            StreamRule::Resolution { values, .. } => StreamRule::Resolution { op: new_op, values },
                            StreamRule::Quality { values, .. }     => StreamRule::Quality { op: new_op, values },
                            StreamRule::Codec { values, .. }      => StreamRule::Codec  { op: new_op, values },
                        };
                    }
                },
                option { value: "in",     selected: !op_not_in, "In" }
                option { value: "not_in", selected:  op_not_in, "Not in" }
            }
            // Value checkboxes
            div { style: "flex:2;display:flex;flex-wrap:wrap;gap:6px;padding-top:2px",
                if field_val == "resolution" {
                    for res in StreamResolution::all() {
                        {
                            let res = res.clone();
                            let checked = match &rule { StreamRule::Resolution { values, .. } => values.contains(&res), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Resolution { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&res) { values.push(res.clone()); } }
                                                else { values.retain(|r| r != &res); }
                                            }
                                        },
                                    }
                                    "{res.label()}"
                                }
                            }
                        }
                    }
                } else if field_val == "quality" {
                    for src in StreamQuality::all() {
                        {
                            let src = src.clone();
                            let checked = match &rule { StreamRule::Quality { values, .. } => values.contains(&src), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Quality { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&src) { values.push(src.clone()); } }
                                                else { values.retain(|s| s != &src); }
                                            }
                                        },
                                    }
                                    "{src.label()}"
                                }
                            }
                        }
                    }
                } else {
                    for codec in StreamCodec::all() {
                        {
                            let codec = codec.clone();
                            let checked = match &rule { StreamRule::Codec { values, .. } => values.contains(&codec), _ => false };
                            rsx! {
                                label { style: "display:flex;align-items:center;gap:3px;font-size:.82rem;cursor:pointer",
                                    input {
                                        r#type: "checkbox",
                                        checked,
                                        onchange: move |e| {
                                            if let Some(StreamRule::Codec { values, .. }) = rules.write().get_mut(idx) {
                                                if e.checked() { if !values.contains(&codec) { values.push(codec.clone()); } }
                                                else { values.retain(|c| c != &codec); }
                                            }
                                        },
                                    }
                                    "{codec.label()}"
                                }
                            }
                        }
                    }
                }
            }
            // Remove button
            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "padding:4px 8px;color:var(--text-muted)",
                onclick: move |_| {
                    let mut r = rules.write();
                    if idx < r.len() { r.remove(idx); }
                },
                "✕"
            }
        }
    }
}

#[component]
pub(crate) fn StreamFilterEditor(
    match_mode: Signal<FilterMatchMode>,
    rules: Signal<Vec<StreamRule>>,
) -> Element {
    let rule_count = rules
        .read()
        .len();
    rsx! {
        div {
            style: "background:var(--bg);border:1px solid var(--border);border-left:3px solid var(--warning);border-radius:8px;padding:12px 14px",
            div { style: "display:flex;align-items:center;justify-content:space-between;margin-bottom:8px",
                label { class: "field-label", style: "margin:0", "Stream Filters" }
                if rule_count > 1 {
                    div { style: "display:flex;align-items:center;gap:6px",
                        span { style: "font-size:.78rem;color:var(--text-muted)", "Match:" }
                        button {
                            style: "font-size:.72rem;height:26px;padding:0 10px",
                            class: if *match_mode.read() == FilterMatchMode::All { "btn btn-primary" } else { "btn btn-ghost" },
                            onclick: move |_| match_mode.set(FilterMatchMode::All),
                            "All (AND)"
                        }
                        button {
                            style: "font-size:.72rem;height:26px;padding:0 10px",
                            class: if *match_mode.read() == FilterMatchMode::Any { "btn btn-primary" } else { "btn btn-ghost" },
                            onclick: move |_| match_mode.set(FilterMatchMode::Any),
                            "Any (OR)"
                        }
                    }
                }
            }
            for (idx, rule) in rules.read().iter().enumerate() {
                StreamRuleRow { key: "{idx}", idx, rule: rule.clone(), rules }
            }
            button {
                class: "btn btn-ghost",
                style: "margin-top:6px;font-size:.75rem;height:28px",
                onclick: move |_| {
                    rules.write().push(StreamRule::Resolution { op: SetOp::In, values: vec![] });
                },
                "+ Add Filter"
            }
        }
    }
}

#[component]
pub fn StreamGroupsCard(app_state: AppState) -> Element {
    let mut groups: Signal<Vec<StreamGroupDto>> = use_signal(Vec::new);
    let mut show_ungrouped = use_signal(|| true);
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut refresh = use_signal(|| 0_u32);

    // Create modal state
    let mut show_create = use_signal(|| false);
    let mut create_name = use_signal(String::new);
    let mut create_match: Signal<FilterMatchMode> = use_signal(|| FilterMatchMode::All);
    let mut create_rules: Signal<Vec<StreamRule>> = use_signal(Vec::new);
    let mut create_priority = use_signal(|| 0_i64);
    let mut creating = use_signal(|| false);

    // Edit modal state
    let mut id_to_edit: Signal<Option<Uuid>> = use_signal(|| None);
    let mut edit_name = use_signal(String::new);
    let mut edit_match: Signal<FilterMatchMode> = use_signal(|| FilterMatchMode::All);
    let mut edit_rules: Signal<Vec<StreamRule>> = use_signal(Vec::new);
    let mut edit_priority = use_signal(|| 0_i64);
    let mut edit_enabled = use_signal(|| true);
    let mut edit_hidden = use_signal(|| false);
    let mut editing = use_signal(|| false);

    // Delete modal state
    let mut id_to_delete: Signal<Option<Uuid>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    let mut saving_setting = use_signal(|| false);

    // Preview state
    let mut preview_imdb = use_signal(|| "tt0133093".to_string());
    let mut preview_data: Signal<Option<StreamGroupPreviewDto>> = use_signal(|| None);
    let mut preview_loading = use_signal(|| false);
    let mut preview_error: Signal<Option<String>> = use_signal(|| None);

    let app_state_preview = app_state.clone();
    use_effect(move || {
        let imdb = preview_imdb
            .read()
            .clone();
        let _r = *refresh.read();
        if imdb.is_empty() {
            return;
        }
        preview_loading.set(true);
        preview_data.set(None);
        preview_error.set(None);
        let client = app_state_preview
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetStreamGroupPreview { imdb_id: imdb })
                .await
            {
                Ok(data) => {
                    preview_data.set(Some(data));
                }
                Err(e) => {
                    preview_error.set(Some(format!("{e}")));
                }
            }
            preview_loading.set(false);
        });
    });

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            let groups_res = client
                .execute(ListStreamGroups)
                .await;
            let cfg_res = client
                .execute(GetSystemConfiguration)
                .await;
            match (groups_res, cfg_res) {
                (Ok(g), Ok(cfg)) => {
                    show_ungrouped.set(
                        cfg.stream_groups_show_ungrouped
                            .unwrap_or(true),
                    );
                    base_cfg.set(Some(cfg));
                    groups.set(g);
                    error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(Some(format!("Failed to load: {e}")));
                }
            }
            loading.set(false);
        });
    });

    rsx! {
        // Settings card
        div { class: "card", style: "margin-bottom:16px",
            div { class: "card-header",
                span { class: "card-title", "Settings" }
            }
            div { class: "card-body",
                div {
                    class: "flex items-center justify-between",
                    style: "padding:8px 0",
                    div {
                        div { style: "font-size:.85rem;font-weight:500", "Show ungrouped streams" }
                        div { style: "font-size:.75rem;color:var(--text-muted)",
                            "Show streams that don't match any group as individual entries."
                        }
                    }
                    div { class: "flex items-center gap-2",
                        if *saving_setting.read() {
                            span { style: "font-size:.72rem;color:var(--text-muted)", "Saving…" }
                        }
                        input {
                            r#type: "checkbox",
                            checked: *show_ungrouped.read(),
                            disabled: *saving_setting.read(),
                            onchange: {
                                let client = app_state.client.clone();
                                move |e: Event<FormData>| {
                                    let checked = e.checked();
                                    show_ungrouped.set(checked);
                                    let Some(cfg) = base_cfg.peek().clone() else { return };
                                    let updated = ServerConfiguration {
                                        stream_groups_show_ungrouped: Some(checked),
                                        ..cfg
                                    };
                                    saving_setting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        let _ = c.execute(UpdateSystemConfiguration { config: updated }).await;
                                        saving_setting.set(false);
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Groups card
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", "Stream Groups" }
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        create_name.set(String::new());
                        create_match.set(FilterMatchMode::All);
                        create_rules.set(vec![]);
                        create_priority.set(0);
                        show_create.set(true);
                    },
                    "+ New Group"
                }
            }
            div { class: "card-body tight",

                if *loading.read() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else if groups.read().is_empty() {
                    EmptyState { message: "No stream groups — create one to consolidate similar streams." }
                } else {
                    div { class: "row-list",
                        for group in groups.read().clone() {
                            {
                                let gid = group.id;
                                let gid_del = group.id;
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)]",
                                        key: "{group.id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "font-weight:500;font-size:.85rem", "{group.name}" }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:3px;display:flex;flex-wrap:wrap;gap:4px",
                                                for rule in group.filter.rules.iter() {
                                                    {
                                                        let (label, is_excl, color_style) = match rule {
                                                            StreamRule::Resolution { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:var(--accent-subtle,rgba(99,102,241,.12));color:var(--accent,#6366f1);padding:1px 6px;border-radius:4px")
                                                            }
                                                            StreamRule::Quality { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:rgba(0,0,0,0.06);padding:1px 6px;border-radius:4px")
                                                            }
                                                            StreamRule::Codec { op, values } => {
                                                                let lbl = values.iter().map(|v| v.label()).collect::<Vec<_>>().join("/");
                                                                (lbl, matches!(op, SetOp::NotIn), "background:rgba(16,185,129,.12);color:rgb(5,150,105);padding:1px 6px;border-radius:4px")
                                                            }
                                                        };
                                                        let prefix = if is_excl { "NOT " } else { "" };
                                                        rsx! { span { style: "{color_style}", "{prefix}{label}" } }
                                                    }
                                                }
                                                if group.filter.rules.len() > 1 {
                                                    span { style: "color:var(--text-muted);font-style:italic",
                                                        {if group.filter.match_mode == FilterMatchMode::All { "AND" } else { "OR" }}
                                                    }
                                                }
                                                span { style: "color:var(--text-muted)", "priority {group.priority}" }
                                                if !group.enabled {
                                                    span { style: "color:var(--error)", "disabled" }
                                                }
                                            }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    edit_name.set(group.name.clone());
                                                    edit_match.set(group.filter.match_mode.clone());
                                                    edit_rules.set(group.filter.rules.clone());
                                                    edit_priority.set(group.priority);
                                                    edit_enabled.set(group.enabled);
                                                    edit_hidden.set(group.hidden);
                                                    id_to_edit.set(Some(gid));
                                                },
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| id_to_delete.set(Some(gid_del)),
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

        // Preview card — only shown when at least one group is configured
        if !groups.read().is_empty() {
            div { class: "card", style: "margin-top:16px",
                div { class: "card-header",
                    span { class: "card-title", "Example output" }
                }
                div { class: "card-body",
                    div { class: "form-group", style: "margin-bottom:12px",
                        label { style: "font-size:.75rem;font-weight:500;display:block;margin-bottom:4px",
                            "IMDB ID"
                        }
                        input {
                            r#type: "text",
                            class: "input",
                            style: "width:180px",
                            value: "{preview_imdb}",
                            oninput: move |e| preview_imdb.set(e.value()),
                        }
                    }
                    if *preview_loading.read() {
                        div { style: "font-size:.8rem;color:var(--text-muted)", "Loading…" }
                    } else if let Some(ref err) = *preview_error.read() {
                        div { style: "font-size:.8rem;color:var(--error)", "{err}" }
                    } else if let Some(ref data) = *preview_data.read() {
                        div { style: "font-family:monospace;font-size:.78rem;line-height:1.6",
                            if data.groups.is_empty() && data.ungrouped.is_empty() {
                                div { style: "color:var(--text-muted)", "No streams returned for this IMDB ID." }
                            }
                            for group in &data.groups {
                                div { style: "margin-bottom:6px",
                                    div { style: "font-weight:600;display:flex;align-items:center;gap:6px",
                                        "▼ {group.name}"
                                        if group.hidden {
                                            span {
                                                style: "font-size:.68rem;padding:1px 5px;border-radius:3px;background:var(--bg-subtle,#333);color:var(--text-muted);font-family:sans-serif",
                                                "hidden"
                                            }
                                        }
                                    }
                                    for stream in &group.streams {
                                        div { style: "padding-left:16px;color:var(--text-muted)",
                                            "└ {stream}"
                                        }
                                    }
                                }
                            }
                            if !data.ungrouped.is_empty() {
                                div { style: "margin-top:4px",
                                    div { style: "font-weight:600", "─ Ungrouped" }
                                    for stream in &data.ungrouped {
                                        div { style: "padding-left:16px;color:var(--text-muted)",
                                            "└ {stream}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Create modal
        if *show_create.read() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "New Stream Group" }
                    }
                    div { class: "modal-body",
                        FormGroup { label: "Name",
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "Auto-generated from filter",
                                value: "{create_name}",
                                oninput: move |e| create_name.set(e.value()),
                            }
                        }
                        FormGroup { label: "Filter rules",
                            StreamFilterEditor { match_mode: create_match, rules: create_rules }
                        }
                        FormGroup { label: "Priority (lower = shown first)",
                            input {
                                class: "form-input",
                                r#type: "number",
                                value: "{create_priority}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        create_priority.set(n);
                                    }
                                },
                            }
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            onclick: move |_| show_create.set(false),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            disabled: *creating.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let name = create_name.read().trim().to_string();
                                    creating.set(true);
                                    let c = client.clone();
                                    let filter = StreamFilter {
                                        match_mode: create_match.peek().clone(),
                                        rules: create_rules.peek().clone(),
                                    };
                                    let prio = *create_priority.peek();
                                    spawn(async move {
                                        match c.execute(CreateStreamGroup {
                                            payload: CreateStreamGroupRequest {
                                                name,
                                                filter,
                                                priority: prio,
                                            },
                                        }).await {
                                            Ok(_) => {
                                                show_create.set(false);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to create: {e}")));
                                                show_create.set(false);
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

        // Edit modal
        if id_to_edit.read().is_some() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Edit Stream Group" }
                    }
                    div { class: "modal-body",
                        FormGroup { label: "Name",
                            input {
                                class: "form-input",
                                r#type: "text",
                                value: "{edit_name}",
                                oninput: move |e| edit_name.set(e.value()),
                            }
                        }
                        FormGroup { label: "Filter rules",
                            StreamFilterEditor { match_mode: edit_match, rules: edit_rules }
                        }
                        FormGroup { label: "Priority (lower = shown first)",
                            input {
                                class: "form-input",
                                r#type: "number",
                                value: "{edit_priority}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        edit_priority.set(n);
                                    }
                                },
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *edit_enabled.read(),
                                    onchange: move |e| edit_enabled.set(e.checked()),
                                }
                                "Enabled"
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *edit_hidden.read(),
                                    onchange: move |e| edit_hidden.set(e.checked()),
                                }
                                "Hide group"
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
                            disabled: *editing.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let Some(id) = *id_to_edit.peek() else { return };
                                    let name = edit_name.read().trim().to_string();
                                    editing.set(true);
                                    let c = client.clone();
                                    let filter = StreamFilter {
                                        match_mode: edit_match.peek().clone(),
                                        rules: edit_rules.peek().clone(),
                                    };
                                    let prio = *edit_priority.peek();
                                    let enabled = *edit_enabled.peek();
                                    let hidden = *edit_hidden.peek();
                                    spawn(async move {
                                        match c.execute(UpdateStreamGroup {
                                            id,
                                            payload: UpdateStreamGroupRequest {
                                                name,
                                                filter,
                                                priority: prio,
                                                enabled,
                                                hidden,
                                            },
                                        }).await {
                                            Ok(_) => {
                                                id_to_edit.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to update: {e}")));
                                                id_to_edit.set(None);
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

        // Delete confirm modal
        if id_to_delete.read().is_some() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    div { class: "modal-header",
                        span { class: "modal-title", "Delete Stream Group" }
                    }
                    div { class: "modal-body",
                        p { style: "font-size:.85rem",
                            "Are you sure you want to delete this stream group? This cannot be undone."
                        }
                    }
                    div { class: "modal-footer",
                        button {
                            class: "btn btn-ghost",
                            disabled: *deleting.read(),
                            onclick: move |_| id_to_delete.set(None),
                            "Cancel"
                        }
                        button {
                            class: "btn btn-primary",
                            style: "background:var(--error);border-color:var(--error)",
                            disabled: *deleting.read(),
                            onclick: {
                                let client = app_state.client.clone();
                                move |_| {
                                    let Some(id) = *id_to_delete.peek() else { return };
                                    deleting.set(true);
                                    let c = client.clone();
                                    spawn(async move {
                                        match c.execute(DeleteStreamGroup { id }).await {
                                            Ok(_) => {
                                                id_to_delete.set(None);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            }
                                            Err(e) => {
                                                error.set(Some(format!("Failed to delete: {e}")));
                                                id_to_delete.set(None);
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
