use crate::state::AppState;
use dioxus::prelude::*;
use remux_sdks::{
    remux::{
        FilterGroup, FilterMatchMode, FilterRule, GetAddonCatalogs,
        GetCertificationSuggestions, GetCountrySuggestions, GetLanguageSuggestions,
        GetLocalSuggestions, GetParentalRatings, GetTagSuggestions, JellyfinAuth,
        ListAddons, NumericOp, ParentalRating, SetOp,
    },
    RestClient,
};
use uuid::Uuid;

fn rule_values(rule: &FilterRule) -> Vec<String> {
    match rule {
        FilterRule::Genre { values, .. }
        | FilterRule::Certification { values, .. }
        | FilterRule::Tag { values, .. }
        | FilterRule::Studio { values, .. }
        | FilterRule::Country { values, .. }
        | FilterRule::OriginalLanguage { values, .. }
        | FilterRule::Person { values, .. } => values.clone(),
        FilterRule::Catalog { catalog_ids, .. } => catalog_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
        _ => vec![],
    }
}

fn is_set_field(key: &str) -> bool {
    matches!(
        key,
        "genre"
            | "certification"
            | "tag"
            | "studio"
            | "country"
            | "original_language"
            | "person"
    )
}

async fn fetch_suggestions(
    client: &RestClient<JellyfinAuth>,
    field: &str,
    query: &str,
) -> Vec<(String, String)> {
    match field {
        "genre" | "studio" | "person" => {
            let kind = match field {
                "genre" => "Genre",
                "studio" => "Studio",
                _ => "Person",
            };
            match client
                .execute(GetLocalSuggestions {
                    kind: kind.into(),
                    search_term: query.into(),
                })
                .await
            {
                Ok(r) => r
                    .items
                    .into_iter()
                    .filter_map(|i| i.name)
                    .map(|n| (n.clone(), n))
                    .collect(),
                Err(_) => vec![],
            }
        }
        "tag" => match client
            .execute(GetTagSuggestions {
                search_term: query.into(),
            })
            .await
        {
            Ok(tags) => tags
                .into_iter()
                .map(|t| (t.clone(), t))
                .collect(),
            Err(_) => vec![],
        },
        "certification" => match client
            .execute(GetCertificationSuggestions {
                search_term: query.into(),
            })
            .await
        {
            Ok(v) => v
                .into_iter()
                .map(|s| (s.clone(), s))
                .collect(),
            Err(_) => vec![],
        },
        "country" => match client
            .execute(GetCountrySuggestions {
                search_term: query.into(),
            })
            .await
        {
            Ok(names) => names
                .into_iter()
                .map(|n| (n.clone(), n))
                .collect(),
            Err(_) => vec![],
        },
        "original_language" => match client
            .execute(GetLanguageSuggestions {
                search_term: query.into(),
            })
            .await
        {
            Ok(langs) => langs
                .into_iter()
                .map(|l| (l.clone(), l))
                .collect(),
            Err(_) => vec![],
        },
        "catalog" => {
            let Ok(addons) = client
                .execute(ListAddons)
                .await
            else {
                return vec![];
            };
            let q_lower = query.to_lowercase();
            let mut results = vec![];
            for addon in addons {
                if !addon.enabled {
                    continue;
                }
                let Ok(catalogs) = client
                    .execute(GetAddonCatalogs { id: addon.id })
                    .await
                else {
                    continue;
                };
                for cat in catalogs {
                    if !cat.enabled {
                        continue;
                    }
                    let Some(cid) = cat.collection_id else {
                        continue;
                    };
                    let label = format!("{} — {}", addon.name, cat.name);
                    if label
                        .to_lowercase()
                        .contains(&q_lower)
                    {
                        results.push((label, cid.to_string()));
                    }
                }
            }
            results
        }
        _ => vec![],
    }
}

fn field_label(key: &str) -> &'static str {
    match key {
        "genre" => "Genre",
        "year" => "Year",
        "rating_audience" => "Audience Rating",
        "rating_critic" => "Critic Rating",
        "parental_rating" => "Max Parental Rating",
        "certification" => "Certification",
        "tag" => "Tag",
        "studio" => "Studio",
        "has_trailer" => "Has Trailer",
        "country" => "Country",
        "original_language" => "Original Language",
        "person" => "Person",
        "catalog" => "Catalog",
        _ => "",
    }
}

fn ops_for_field(field_key: &str) -> Vec<(&'static str, &'static str)> {
    match field_key {
        "year" | "rating_audience" | "rating_critic" => {
            vec![("eq", "is"), ("not_eq", "is not"), ("gt", ">"), ("lt", "<")]
        }
        "parental_rating" | "has_trailer" => vec![],
        _ => vec![("is", "is"), ("is_not", "is not")],
    }
}

fn value_placeholder(field_key: &str) -> &'static str {
    match field_key {
        "year" => "2020",
        "rating_audience" | "rating_critic" => "7.5",
        "parental_rating" => "13",
        "certification" => "PG-13",
        "country" => "United States of America",
        "original_language" => "en",
        _ => "Action, Horror",
    }
}

fn rule_to_raw(rule: &FilterRule) -> (String, String, String) {
    match rule {
        FilterRule::Catalog { op, catalog_ids } => {
            let val = catalog_ids
                .iter()
                .filter(|id| !id.is_nil())
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            ("catalog".into(), set_op_str(op), val)
        }
        FilterRule::Year { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("year".into(), op_str.into(), value.to_string())
        }
        FilterRule::RatingAudience { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("rating_audience".into(), op_str.into(), value.to_string())
        }
        FilterRule::RatingCritic { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("rating_critic".into(), op_str.into(), value.to_string())
        }
        FilterRule::ParentalRating { op, value } => {
            let op_str = match op {
                NumericOp::Eq => "eq",
                NumericOp::NotEq => "not_eq",
                NumericOp::Gt => "gt",
                NumericOp::Lt => "lt",
            };
            ("parental_rating".into(), op_str.into(), value.to_string())
        }
        FilterRule::Genre { op, values } => {
            ("genre".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Certification { op, values } => {
            ("certification".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Tag { op, values } => {
            ("tag".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Studio { op, values } => {
            ("studio".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::Country { op, values } => {
            ("country".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::OriginalLanguage { op, values } => (
            "original_language".into(),
            set_op_str(op),
            values.join(", "),
        ),
        FilterRule::Person { op, values } => {
            ("person".into(), set_op_str(op), values.join(", "))
        }
        FilterRule::HasTrailer { value } => {
            ("has_trailer".into(), String::new(), value.to_string())
        }
    }
}

fn set_op_str(op: &SetOp) -> String {
    match op {
        SetOp::Is | SetOp::In => "is",
        SetOp::IsNot | SetOp::NotIn => "is_not",
    }
    .into()
}

fn raw_to_rule(field: &str, op: &str, value_str: &str) -> FilterRule {
    let set_op = match op {
        "is_not" => SetOp::NotIn,
        _ => SetOp::In,
    };
    let num_op = match op {
        "not_eq" => NumericOp::NotEq,
        "gt" => NumericOp::Gt,
        "lt" => NumericOp::Lt,
        _ => NumericOp::Eq,
    };
    let set_values = || -> Vec<String> {
        value_str
            .split(',')
            .map(|s| {
                s.trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect()
    };

    match field {
        "year" => FilterRule::Year {
            op: num_op,
            value: value_str
                .parse()
                .unwrap_or(0),
        },
        "rating_audience" => FilterRule::RatingAudience {
            op: num_op,
            value: value_str
                .parse()
                .unwrap_or(0.0),
        },
        "rating_critic" => FilterRule::RatingCritic {
            op: num_op,
            value: value_str
                .parse()
                .unwrap_or(0.0),
        },
        "parental_rating" => FilterRule::ParentalRating {
            op: NumericOp::Lt,
            value: value_str
                .parse()
                .unwrap_or(0),
        },
        "certification" => FilterRule::Certification {
            op: set_op,
            values: set_values(),
        },
        "tag" => FilterRule::Tag {
            op: set_op,
            values: set_values(),
        },
        "studio" => FilterRule::Studio {
            op: set_op,
            values: set_values(),
        },
        "country" => FilterRule::Country {
            op: set_op,
            values: set_values(),
        },
        "original_language" => FilterRule::OriginalLanguage {
            op: set_op,
            values: set_values(),
        },
        "person" => FilterRule::Person {
            op: set_op,
            values: set_values(),
        },
        "has_trailer" => FilterRule::HasTrailer {
            value: value_str == "true",
        },
        "catalog" => FilterRule::Catalog {
            op: set_op,
            catalog_ids: value_str
                .split(", ")
                .filter_map(|s| Uuid::parse_str(s.trim()).ok())
                .collect(),
        },
        _ => FilterRule::Genre {
            op: set_op,
            values: set_values(),
        },
    }
}

#[component]
pub fn TagChipInput(tags: Signal<Vec<String>>) -> Element {
    let app_state = use_context::<AppState>();
    let mut input_text: Signal<String> = use_signal(String::new);
    let mut suggestions: Signal<Vec<String>> = use_signal(Vec::new);
    let mut show_dropdown = use_signal(|| false);

    let client_fetch = app_state
        .client
        .clone();
    use_effect(move || {
        let q = input_text
            .read()
            .clone();
        let client = client_fetch.clone();
        spawn(async move {
            if q.is_empty() {
                suggestions.set(vec![]);
                show_dropdown.set(false);
                return;
            }
            match client
                .execute(GetTagSuggestions { search_term: q })
                .await
            {
                Ok(v) => {
                    show_dropdown.set(!v.is_empty());
                    suggestions.set(v);
                }
                Err(_) => {}
            }
        });
    });

    let mut add_tag = move |tag: String| {
        let tag = tag
            .trim()
            .to_string();
        if !tag.is_empty()
            && !tags
                .read()
                .contains(&tag)
        {
            tags.write()
                .push(tag);
        }
        input_text.set(String::new());
        suggestions.set(vec![]);
        show_dropdown.set(false);
    };

    rsx! {
        div { style: "position:relative",
            div { class: "chip-input",
                for (ci, chip) in tags.read().clone().into_iter().enumerate() {
                    span { class: "chip", key: "{ci}",
                        "{chip}"
                        button {
                            r#type: "button",
                            class: "chip-remove",
                            onclick: move |_| { tags.write().remove(ci); },
                            "×"
                        }
                    }
                }
                input {
                    r#type: "text",
                    class: "chip-text-input",
                    placeholder: if tags.read().is_empty() { "add tag…" } else { "" },
                    value: "{input_text}",
                    oninput: move |e| input_text.set(e.value()),
                    onkeydown: move |e| {
                        let key = e.key().to_string();
                        let text = input_text.read().replace(',', "").trim().to_string();
                        if (key == "Enter" || key == ",") && !text.is_empty() {
                            e.prevent_default();
                            add_tag(text);
                        } else if key == "Backspace" && input_text.read().is_empty() {
                            tags.write().pop();
                        }
                    },
                }
            }
            if *show_dropdown.read() {
                div { class: "autocomplete-dropdown",
                    for (si, suggestion) in suggestions.read().clone().into_iter().enumerate() {
                        div {
                            class: "autocomplete-item",
                            key: "{si}",
                            onmousedown: move |e| {
                                e.prevent_default();
                                add_tag(suggestion.clone());
                            },
                            "{suggestion}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ChipInput(
    field_key: String,
    op_val: String,
    values: Vec<String>,
    idx: usize,
    rules: Signal<Vec<FilterRule>>,
    /// A signal of (display_label, value) pairs for looking up human-readable chip labels.
    /// ChipInput subscribes to this signal directly, so it re-renders when the data loads.
    #[props(default)]
    value_labels: Option<Signal<Vec<(String, String)>>>,
) -> Element {
    let app_state = use_context::<AppState>();
    let mut input_text: Signal<String> = use_signal(String::new);
    let mut suggestions: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut show_dropdown = use_signal(|| false);
    let mut label_cache: Signal<std::collections::HashMap<String, String>> =
        use_signal(std::collections::HashMap::new);

    let fk_fetch = field_key.clone();
    let client_fetch = app_state
        .client
        .clone();
    use_effect(move || {
        let q = input_text
            .read()
            .clone();
        let fk = fk_fetch.clone();
        let client = client_fetch.clone();
        spawn(async move {
            if q.is_empty() {
                suggestions.set(vec![]);
                show_dropdown.set(false);
                return;
            }
            let result = fetch_suggestions(&client, &fk, &q).await;
            show_dropdown.set(!result.is_empty());
            suggestions.set(result);
        });
    });

    // Build display map before rsx! so subscriptions are tracked at component scope.
    // 1. Start with externally-provided labels (signal read → ChipInput subscribes directly).
    // 2. Extend with label_cache (dropdown selections override external labels).
    let chip_labels = {
        let mut m: std::collections::HashMap<String, String> =
            if let Some(vl) = value_labels {
                // (label, value) pairs — invert to (value, label) for lookup
                vl.read()
                    .iter()
                    .map(|(label, value)| (value.clone(), label.clone()))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
        m.extend(
            label_cache
                .read()
                .clone(),
        );
        m
    };

    rsx! {
        div { style: "position:relative;flex:2 1 130px;min-width:130px",
            div { class: "chip-input",
                for (ci, chip) in values.iter().enumerate() {
                    {
                        let chip_display = chip_labels.get(chip).cloned().unwrap_or(chip.clone());
                        let mut v = values.clone();
                        let fk = field_key.clone();
                        let op = op_val.clone();
                        rsx! {
                            span { class: "chip", key: "{ci}",
                                span { class: "chip-label", title: "{chip_display}", "{chip_display}" }
                                button {
                                    r#type: "button",
                                    class: "chip-remove",
                                    onclick: move |_| {
                                        v.remove(ci);
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk, &op, &v.join(", "));
                                        }
                                    },
                                    "×"
                                }
                            }
                        }
                    }
                }
                {
                    let fk_kd = field_key.clone();
                    let op_kd = op_val.clone();
                    let vals_kd = values.clone();
                    rsx! {
                        input {
                            r#type: "text",
                            class: "chip-text-input",
                            placeholder: if values.is_empty() { "type to search…" } else { "" },
                            value: "{input_text}",
                            oninput: move |e| input_text.set(e.value()),
                            onkeydown: move |e| {
                                let key = e.key().to_string();
                                let text = input_text.read().replace(',', "");
                                let text = text.trim().to_string();
                                if (key == "Enter" || key == ",") && !text.is_empty() {
                                    e.prevent_default();
                                    let mut v = vals_kd.clone();
                                    if !v.contains(&text) { v.push(text); }
                                    if let Some(row) = rules.write().get_mut(idx) {
                                        *row = raw_to_rule(&fk_kd, &op_kd, &v.join(", "));
                                    }
                                    input_text.set(String::new());
                                    suggestions.set(vec![]);
                                    show_dropdown.set(false);
                                } else if key == "Backspace" && input_text.read().is_empty() {
                                    let mut v = vals_kd.clone();
                                    if !v.is_empty() {
                                        v.pop();
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk_kd, &op_kd, &v.join(", "));
                                        }
                                    }
                                }
                            },
                            onblur: move |_| show_dropdown.set(false),
                            onfocus: move |_| {
                                if !suggestions.read().is_empty() { show_dropdown.set(true); }
                            },
                        }
                    }
                }
            }
            if *show_dropdown.read() {
                div {
                    class: "chip-dropdown",
                    onmousedown: |e| e.prevent_default(),
                    for (label, value) in suggestions.read().clone() {
                        {
                            let mut v = values.clone();
                            let fk = field_key.clone();
                            let op = op_val.clone();
                            rsx! {
                                div {
                                    class: "chip-dropdown-item",
                                    key: "{value}",
                                    onmousedown: move |_| {
                                        label_cache.write().insert(value.clone(), label.clone());
                                        if !v.contains(&value) { v.push(value.clone()); }
                                        if let Some(row) = rules.write().get_mut(idx) {
                                            *row = raw_to_rule(&fk, &op, &v.join(", "));
                                        }
                                        input_text.set(String::new());
                                        suggestions.set(vec![]);
                                        show_dropdown.set(false);
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn FilterRuleRow(
    idx: usize,
    rule: FilterRule,
    rules: Signal<Vec<FilterRule>>,
) -> Element {
    let app_state = use_context::<AppState>();
    let client_for_ratings = app_state
        .client
        .clone();
    let client_for_catalogs = app_state
        .client
        .clone();
    let mut parental_ratings: Signal<Vec<ParentalRating>> = use_signal(Vec::new);
    use_effect(move || {
        let client = client_for_ratings.clone();
        spawn(async move {
            if let Ok(ratings) = client
                .execute(GetParentalRatings)
                .await
            {
                parental_ratings.set(ratings);
            }
        });
    });

    let mut catalog_options: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    use_effect(move || {
        let client = client_for_catalogs.clone();
        spawn(async move {
            let Ok(addons) = client
                .execute(ListAddons)
                .await
            else {
                return;
            };
            let mut options = vec![];
            for addon in addons {
                let Ok(catalogs) = client
                    .execute(GetAddonCatalogs { id: addon.id })
                    .await
                else {
                    continue;
                };
                for cat in catalogs {
                    if let Some(cid) = cat.collection_id {
                        let disabled = !addon.enabled || !cat.enabled;
                        let label = if disabled {
                            format!("{} — {} (disabled)", addon.name, cat.name)
                        } else {
                            format!("{} — {}", addon.name, cat.name)
                        };
                        options.push((label, cid.to_string()));
                    }
                }
            }
            catalog_options.set(options);
        });
    });

    let (field_val, op_val, value_val) = rule_to_raw(&rule);
    let ops = ops_for_field(&field_val);
    let is_trailer = field_val == "has_trailer";
    let is_parental_rating = field_val == "parental_rating";
    let is_catalog = field_val == "catalog";
    let hide_operator = is_trailer || is_parental_rating;

    let fv1 = field_val.clone();
    let fv2 = field_val.clone();
    let ov1 = op_val.clone();
    let vv1 = value_val.clone();
    let vv2 = value_val.clone();

    let grouped_ratings: Vec<(i32, String)> = {
        let ratings = parental_ratings.read();
        let mut groups: Vec<(i32, i32, String)> = vec![];
        for rating in ratings
            .iter()
            .filter(|r| {
                r.value
                    .is_some()
            })
        {
            let score = rating
                .value
                .unwrap();
            let sub = rating
                .rating_score
                .as_ref()
                .and_then(|s| s.sub_score)
                .unwrap_or(0);
            if let Some(last) = groups.last_mut() {
                if last.0 == score && last.1 == sub {
                    last.2
                        .push('/');
                    last.2
                        .push_str(&rating.name);
                    continue;
                }
            }
            groups.push((
                score,
                sub,
                rating
                    .name
                    .clone(),
            ));
        }
        groups
            .into_iter()
            .map(|(score, _, label)| (score, label))
            .collect()
    };

    let field_style = if hide_operator {
        "flex:1 1 100%;min-width:110px"
    } else {
        "flex:1 1 110px;min-width:110px"
    };
    rsx! {
        div { style: "display:flex;align-items:flex-start;gap:6px;flex-wrap:wrap",
            select {
                class: "select-input",
                style: "{field_style}",
                value: "{field_val}",
                onchange: move |e| {
                    let new_field = e.value();
                    let default_op = ops_for_field(&new_field).first().map(|(v, _)| *v).unwrap_or("");
                    if let Some(row) = rules.write().get_mut(idx) {
                        *row = raw_to_rule(&new_field, default_op, "");
                    }
                },
                option { value: "genre",           selected: field_val == "genre",           { field_label("genre") } }
                option { value: "year",            selected: field_val == "year",            { field_label("year") } }
                option { value: "rating_audience", selected: field_val == "rating_audience", { field_label("rating_audience") } }
                option { value: "rating_critic",   selected: field_val == "rating_critic",   { field_label("rating_critic") } }
                option { value: "parental_rating", selected: field_val == "parental_rating", { field_label("parental_rating") } }
                option { value: "tag",             selected: field_val == "tag",             { field_label("tag") } }
                option { value: "studio",          selected: field_val == "studio",          { field_label("studio") } }
                option { value: "has_trailer",     selected: field_val == "has_trailer",     { field_label("has_trailer") } }
                option { value: "country",            selected: field_val == "country",            { field_label("country") } }
                option { value: "original_language", selected: field_val == "original_language", { field_label("original_language") } }
                option { value: "person",             selected: field_val == "person",             { field_label("person") } }
                option { value: "catalog",         selected: field_val == "catalog",         { field_label("catalog") } }
            }
            if !hide_operator {
                select {
                    class: "select-input",
                    style: "flex:1 1 80px;min-width:80px",
                    value: "{op_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv1, &e.value(), &vv1);
                        }
                    },
                    for (op_v, op_l) in ops.iter() {
                        option { value: *op_v, selected: op_val == *op_v, "{op_l}" }
                    }
                }
            }
            if is_catalog {
                ChipInput {
                    field_key: "catalog".to_string(),
                    op_val: op_val.clone(),
                    values: rule_values(&rule),
                    idx,
                    rules,
                    value_labels: Some(catalog_options),
                }
            } else if is_trailer {
                select {
                    class: "select-input",
                    style: "flex:2 1 130px;min-width:130px",
                    value: "{value_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule("has_trailer", "", &e.value());
                        }
                    },
                    option { value: "true",  selected: value_val == "true",  "Yes" }
                    option { value: "false", selected: value_val == "false", "No" }
                }
            } else if is_parental_rating {
                select {
                    class: "select-input",
                    style: "flex:2 1 130px;min-width:130px",
                    value: "{value_val}",
                    onchange: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv2, "lt", &e.value());
                        }
                    },
                    option { value: "", selected: value_val.is_empty(), disabled: true, "Select rating" }
                    for (score, label) in grouped_ratings {
                        option {
                            value: "{score}",
                            selected: value_val == score.to_string(),
                            "{label}"
                        }
                    }
                }
            } else if is_set_field(&field_val) {
                ChipInput {
                    field_key: field_val.clone(),
                    op_val: op_val.clone(),
                    values: rule_values(&rule),
                    idx,
                    rules,
                }
            } else {
                input {
                    class: "field-input",
                    style: "flex:2 1 130px;min-width:130px",
                    r#type: "text",
                    placeholder: value_placeholder(&fv2),
                    value: "{vv2}",
                    oninput: move |e| {
                        if let Some(row) = rules.write().get_mut(idx) {
                            *row = raw_to_rule(&fv2, &ov1, &e.value());
                        }
                    },
                }
            }
            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "padding:4px 8px;color:var(--text-muted)",
                onclick: move |_| {
                    let mut r = rules.write();
                    if idx < r.len() {
                        r.remove(idx);
                    }
                },
                "✕"
            }
        }
    }
}

// ── Group editor ─────────────────────────────────────────────────────────────

#[component]
fn FilterGroupRow(
    group_idx: usize,
    group: FilterGroup,
    groups: Signal<Vec<FilterGroup>>,
) -> Element {
    let default_new_rule = FilterRule::Genre {
        op: SetOp::In,
        values: vec![],
    };
    let can_delete = groups
        .read()
        .len()
        > 1;

    // Proxy signals so FilterRuleRow can mutate the group's rules via the groups signal.
    let mut rules: Signal<Vec<FilterRule>> = {
        let g = group.clone();
        use_signal(move || {
            g.rules
                .clone()
        })
    };
    let mut group_match: Signal<FilterMatchMode> = {
        let m = group
            .match_mode
            .clone();
        use_signal(move || m)
    };

    // Sync local signals back into the parent `groups` signal when they change.
    use_effect(move || {
        let r = rules
            .read()
            .clone();
        let m = group_match
            .read()
            .clone();
        let mut gs = groups.write();
        if let Some(g) = gs.get_mut(group_idx) {
            g.rules = r;
            g.match_mode = m;
        }
    });

    rsx! {
        div {
            style: "border:1px solid var(--border);border-radius:6px;padding:10px 12px;background:var(--bg-alt, var(--bg));display:flex;flex-direction:column;gap:6px",

            // Group header: match mode selector + remove button
            div { style: "display:flex;align-items:center;justify-content:space-between",
                div { style: "display:flex;align-items:center;gap:6px",
                    span { style: "font-size:0.75rem;color:var(--text-muted);text-transform:uppercase;letter-spacing:.04em", "Match" }
                    select {
                        class: "select-input",
                        style: "padding:2px 6px;font-size:0.8rem",
                        value: if *group_match.read() == FilterMatchMode::All { "all" } else { "any" },
                        onchange: move |e| {
                            group_match.set(if e.value() == "any" { FilterMatchMode::Any } else { FilterMatchMode::All });
                        },
                        option { value: "all", "All (AND)" }
                        option { value: "any", "Any (OR)" }
                    }
                }
                if can_delete {
                    button {
                        r#type: "button",
                        class: "btn btn-ghost",
                        style: "font-size:0.75rem;color:var(--text-muted);padding:2px 6px",
                        onclick: move |_| {
                            groups.write().remove(group_idx);
                        },
                        "✕ Remove group"
                    }
                }
            }

            // Rules inside this group
            div { style: "display:flex;flex-direction:column;gap:0",
                for (idx, rule) in rules.read().iter().enumerate() {
                    if idx > 0 {
                        div { style: "height:1px;background:var(--border);margin:6px 0;opacity:0.5" }
                    }
                    FilterRuleRow {
                        key: "{idx}",
                        idx,
                        rule: rule.clone(),
                        rules,
                    }
                }
            }

            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "font-size:0.82rem;align-self:flex-start;margin-top:2px",
                onclick: move |_| {
                    rules.write().push(default_new_rule.clone());
                },
                "+ Add rule"
            }
        }
    }
}

// ── Top-level editor ──────────────────────────────────────────────────────────

#[component]
pub fn FilterRuleEditor(
    match_mode: Signal<FilterMatchMode>,
    groups: Signal<Vec<FilterGroup>>,
) -> Element {
    let has_multiple = groups
        .read()
        .len()
        > 1;
    rsx! {
        div {
            style: "background:var(--bg);border:1px solid var(--border);border-left:3px solid var(--info);border-radius:8px;padding:12px 14px",

            // Header: title + group combiner (only shown when >1 group)
            div { style: "display:flex;align-items:center;justify-content:space-between;margin-bottom:10px",
                label { class: "field-label", style: "margin:0", "Media Filters" }
                if has_multiple {
                    div { style: "display:flex;align-items:center;gap:6px",
                        span { style: "font-size:0.8rem;color:var(--text-muted)", "Combine groups" }
                        select {
                            class: "select-input",
                            style: "padding:2px 6px;font-size:0.8rem",
                            value: if *match_mode.read() == FilterMatchMode::All { "all" } else { "any" },
                            onchange: move |e| {
                                match_mode.set(if e.value() == "any" { FilterMatchMode::Any } else { FilterMatchMode::All });
                            },
                            option { value: "all", "AND" }
                            option { value: "any", "OR" }
                        }
                    }
                }
            }

            // Groups
            div { style: "display:flex;flex-direction:column;gap:8px",
                for (idx, group) in groups.read().iter().cloned().enumerate() {
                    FilterGroupRow {
                        key: "{idx}",
                        group_idx: idx,
                        group,
                        groups,
                    }
                }
            }

            button {
                r#type: "button",
                class: "btn btn-ghost",
                style: "margin-top:10px;font-size:0.85rem",
                onclick: move |_| {
                    groups.write().push(FilterGroup::default());
                },
                "+ Add group"
            }
        }
    }
}
