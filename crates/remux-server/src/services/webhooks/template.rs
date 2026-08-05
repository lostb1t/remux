//! Handlebars rendering for webhook bodies.
//!
//! Templates are compiled once per reload and rendered by name afterwards: the
//! registry cached in [`super::LoadedWebhooks`] only pays off if nothing
//! re-parses the template string per event.
//!
//! The five custom helpers mirror the Jellyfin webhook plugin's, so templates
//! written for it keep working.

use crate::db;
use handlebars::{
    Context, Handlebars, Helper, HelperResult, Output, RenderContext,
    RenderErrorReason, Renderable,
};
use serde_json::{Map, Value};
use tracing::warn;

/// An empty registry that already knows the custom helpers.
///
/// Both the startup snapshot and every reload go through here — a registry
/// built any other way silently loses the helpers.
pub(crate) fn fresh_registry() -> Handlebars<'static> {
    let mut registry = Handlebars::new();
    // Missing variables render as empty rather than failing the whole body:
    // most variables are event-specific and templates are user-written.
    registry.set_strict_mode(false);
    // Bodies are JSON, not HTML: `{{Var}}` almost always sits inside a JSON
    // string literal, so that is what values are escaped for. HTML escaping
    // would mangle `Ocean's` into `Ocean&#x27;s`, and no escaping at all would
    // let a title like `The "Burbs` break the body. `{{{Var}}}` stays the raw
    // escape hatch, exactly as in the Jellyfin plugin's stock templates.
    registry.register_escape_fn(escape_json_string);
    register_helpers(&mut registry);
    registry
}

/// Escape `value` for insertion inside a JSON string literal: the JSON
/// encoding of the string, minus its surrounding quotes.
fn escape_json_string(value: &str) -> String {
    let encoded = Value::String(value.to_string()).to_string();
    // `Value::String` always serializes as `"…"`, so both quotes are present.
    encoded[1..encoded.len() - 1].to_string()
}

/// A registry with every hook's template pre-compiled under its id.
///
/// A hook whose template does not parse is skipped, not fatal: the others must
/// still be delivered. `render` then reports the missing template per event.
pub(crate) fn build_registry(hooks: &[db::Webhook]) -> Handlebars<'static> {
    let mut registry = fresh_registry();
    for hook in hooks {
        if let Err(e) = registry.register_template_string(
            &hook
                .id
                .to_string(),
            &hook.template,
        ) {
            warn!(webhook = %hook.name, error = %e, "invalid webhook template, hook will not render");
        }
    }
    registry
}

/// A registry carrying exactly one hook's template, with the parse error
/// **propagated**.
///
/// [`build_registry`] is deliberately lenient — one hook's typo must not stop
/// the others being delivered — but that leniency turns a syntax error into a
/// later "Template not found: <uuid>" from `render`, naming an id the operator
/// never typed and hiding the real error in the server log. Callers with a
/// single hook in hand and an operator waiting on the answer use this instead.
pub(crate) fn single_registry(
    hook: &db::Webhook,
) -> Result<Handlebars<'static>, handlebars::TemplateError> {
    let mut registry = fresh_registry();
    registry.register_template_string(
        &hook
            .id
            .to_string(),
        &hook.template,
    )?;
    Ok(registry)
}

/// Whether an operator-supplied template parses.
///
/// The error text is derived from the operator's own template — never from a
/// remote response — so it is safe to hand back over the API.
/// The name the template is registered under while it is being checked. It
/// appears in handlebars' error text, so it has to read as something the
/// operator recognises rather than as an internal id.
const VALIDATION_NAME: &str = "webhook template";

pub(crate) fn validate(template: &str) -> Result<(), handlebars::TemplateError> {
    Handlebars::new().register_template_string(VALIDATION_NAME, template)
}

pub(crate) fn register_helpers(registry: &mut Handlebars<'_>) {
    registry.register_helper("if_equals", Box::new(if_equals));
    registry.register_helper("if_exist", Box::new(if_exist));
    registry.register_helper("link_to", Box::new(link_to));
    registry.register_helper("url_encode", Box::new(url_encode));
    registry.register_helper("json_encode", Box::new(json_encode));
}

/// Render `hook`'s body for `data`, or `None` when the hook asked for empty
/// bodies to be dropped.
pub(crate) fn render(
    hook: &db::Webhook,
    registry: &Handlebars<'static>,
    data: &Map<String, Value>,
) -> anyhow::Result<Option<String>> {
    let data = super::payload::with_hook_fields(data, hook);

    let body = if hook.send_all_properties {
        // The whole dictionary, template ignored — this is the "show me every
        // variable" mode of the plugin.
        serde_json::to_string_pretty(data.as_ref())?
    } else {
        registry.render(
            &hook
                .id
                .to_string(),
            data.as_ref(),
        )?
    };

    let body = if hook.trim_whitespace {
        body.trim()
            .to_string()
    } else {
        body
    };

    if hook.skip_empty_message_body
        && body
            .trim()
            .is_empty()
    {
        return Ok(None);
    }
    Ok(Some(body))
}

// --- helpers --------------------------------------------------------------

/// `{{#if_equals A B}}…{{else}}…{{/if_equals}}` — case-insensitive comparison
/// of the two parameters rendered as strings.
fn if_equals<'reg, 'rc>(
    h: &Helper<'rc>,
    registry: &'reg Handlebars<'reg>,
    ctx: &'rc Context,
    rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let lhs = required_param(h, 0, "if_equals")?;
    let rhs = required_param(h, 1, "if_equals")?;
    let branch = if lhs.eq_ignore_ascii_case(&rhs) {
        h.template()
    } else {
        h.inverse()
    };
    if let Some(template) = branch {
        template.render(registry, ctx, rc, out)?;
    }
    Ok(())
}

/// `{{#if_exist A}}…{{else}}…{{/if_exist}}` — renders the block when the value
/// is present and not empty. Present-but-falsy values (`0`, `false`) exist.
fn if_exist<'reg, 'rc>(
    h: &Helper<'rc>,
    registry: &'reg Handlebars<'reg>,
    ctx: &'rc Context,
    rc: &mut RenderContext<'reg, 'rc>,
    out: &mut dyn Output,
) -> HelperResult {
    let exists = h
        .param(0)
        .map(|param| param.value())
        .is_some_and(|value| match value {
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            Value::Bool(_) | Value::Number(_) => true,
        });
    let branch = if exists { h.template() } else { h.inverse() };
    if let Some(template) = branch {
        template.render(registry, ctx, rc, out)?;
    }
    Ok(())
}

/// `{{link_to url text}}` → `<a href='url'>text</a>`.
///
/// Single quotes on purpose, as in the plugin: the tag has to survive inside a
/// JSON string literal, which double quotes would terminate.
fn link_to(
    h: &Helper,
    _registry: &Handlebars,
    _ctx: &Context,
    _rc: &mut RenderContext,
    out: &mut dyn Output,
) -> HelperResult {
    let url = required_param(h, 0, "link_to")?;
    let text = required_param(h, 1, "link_to")?;
    out.write(&format!("<a href='{url}'>{text}</a>"))?;
    Ok(())
}

/// `{{url_encode value}}` — percent-encoding, for building query strings.
fn url_encode(
    h: &Helper,
    _registry: &Handlebars,
    _ctx: &Context,
    _rc: &mut RenderContext,
    out: &mut dyn Output,
) -> HelperResult {
    let value = required_param(h, 0, "url_encode")?;
    out.write(&urlencoding::encode(&value))?;
    Ok(())
}

/// `{{json_encode value}}` — the value escaped for a JSON string literal,
/// **without** surrounding quotes.
///
/// The plugin idiom is `"title": "{{json_encode Name}}"`, i.e. the template
/// supplies the quotes: emitting them here would produce `""…""` and an
/// invalid body.
fn json_encode(
    h: &Helper,
    _registry: &Handlebars,
    _ctx: &Context,
    _rc: &mut RenderContext,
    out: &mut dyn Output,
) -> HelperResult {
    let value = h
        .param(0)
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("json_encode", 0))?
        .value();
    let encoded = match value {
        // Non-string values are already valid JSON literals as they stand.
        Value::String(s) => escape_json_string(s),
        other => serde_json::to_string(other)
            .map_err(|e| RenderErrorReason::NestedError(Box::new(e)))?,
    };
    out.write(&encoded)?;
    Ok(())
}

/// A parameter rendered the way a template would render it: strings as-is,
/// everything else as its JSON form, missing values as empty.
fn required_param(
    h: &Helper,
    index: usize,
    helper: &'static str,
) -> Result<String, RenderErrorReason> {
    let value = h
        .param(index)
        .ok_or(RenderErrorReason::ParamNotFoundForIndex(helper, index))?
        .value();
    Ok(match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::remux::{
        DiscordMentionType, NotificationType, WebhookDestination, WebhookItemTypes,
        WebhookKeyValue,
    };
    use serde_json::json;
    use uuid::Uuid;

    fn hook(template: &str) -> db::Webhook {
        let now = chrono::Utc::now();
        db::Webhook {
            id: Uuid::from_u128(100),
            name: "test".into(),
            enabled: true,
            url: "https://example.test/hook".into(),
            template: template.into(),
            destination: WebhookDestination::Discord {
                avatar_url: None,
                bot_username: None,
                embed_color: None,
                mention_type: DiscordMentionType::None,
            },
            notification_types: vec![NotificationType::ItemAdded],
            user_filter: vec![],
            item_types: WebhookItemTypes::default(),
            send_all_properties: false,
            trim_whitespace: false,
            skip_empty_message_body: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn data(pairs: Value) -> Map<String, Value> {
        pairs
            .as_object()
            .expect("test data must be an object")
            .clone()
    }

    /// Renders `template` against `pairs` through the real registry path
    /// (pre-compiled, registered under the hook id).
    fn render_template(template: &str, pairs: Value) -> String {
        let hook = hook(template);
        let registry = build_registry(std::slice::from_ref(&hook));
        render(&hook, &registry, &data(pairs))
            .expect("render must succeed")
            .expect("render must produce a body")
    }

    // --- if_equals --------------------------------------------------------

    const IF_EQUALS: &str =
        "{{#if_equals ItemType \"episode\"}}yes{{else}}no{{/if_equals}}";

    #[test]
    fn if_equals_ignores_case() {
        assert_eq!(
            render_template(IF_EQUALS, json!({ "ItemType": "Episode" })),
            "yes"
        );
        assert_eq!(
            render_template(IF_EQUALS, json!({ "ItemType": "EPISODE" })),
            "yes"
        );
    }

    #[test]
    fn if_equals_takes_the_else_branch_on_different_values() {
        assert_eq!(
            render_template(IF_EQUALS, json!({ "ItemType": "Movie" })),
            "no"
        );
        // A missing value must not accidentally equal the literal.
        assert_eq!(render_template(IF_EQUALS, json!({})), "no");
    }

    #[test]
    fn if_equals_compares_non_string_values() {
        assert_eq!(
            render_template(
                "{{#if_equals SeasonNumber 2}}yes{{else}}no{{/if_equals}}",
                json!({ "SeasonNumber": 2 })
            ),
            "yes"
        );
        assert_eq!(
            render_template(
                "{{#if_equals SeasonNumber 3}}yes{{else}}no{{/if_equals}}",
                json!({ "SeasonNumber": 2 })
            ),
            "no"
        );
    }

    // --- if_exist ---------------------------------------------------------

    const IF_EXIST: &str = "{{#if_exist Overview}}yes{{else}}no{{/if_exist}}";

    #[test]
    fn if_exist_renders_only_for_a_present_non_empty_value() {
        assert_eq!(
            render_template(IF_EXIST, json!({ "Overview": "some text" })),
            "yes"
        );
        // Falsy-but-present values still exist.
        assert_eq!(render_template(IF_EXIST, json!({ "Overview": 0 })), "yes");
        assert_eq!(
            render_template(IF_EXIST, json!({ "Overview": false })),
            "yes"
        );
    }

    #[test]
    fn if_exist_takes_the_else_branch_for_null_empty_and_missing() {
        assert_eq!(
            render_template(IF_EXIST, json!({ "Overview": Value::Null })),
            "no"
        );
        assert_eq!(render_template(IF_EXIST, json!({ "Overview": "" })), "no");
        assert_eq!(render_template(IF_EXIST, json!({})), "no");
    }

    // --- link_to / url_encode / json_encode -------------------------------

    /// Single-quoted `href`, as the plugin emits: the anchor has to survive
    /// inside a JSON string literal, which a double quote would terminate.
    #[test]
    fn link_to_emits_a_single_quoted_anchor() {
        let body = render_template(
            "{{link_to ServerUrl Name}}",
            json!({ "ServerUrl": "https://example.test/web", "Name": "Open" }),
        );
        assert_eq!(body, "<a href='https://example.test/web'>Open</a>");
        assert!(
            !body.contains('"'),
            "a double quote would break the JSON body: {body}"
        );

        // The canonical use: inside a JSON string.
        let json_body = render_template(
            r#"{"content": "{{link_to ServerUrl Name}}"}"#,
            json!({ "ServerUrl": "https://example.test/web", "Name": "Open" }),
        );
        serde_json::from_str::<Value>(&json_body)
            .unwrap_or_else(|e| panic!("{json_body} must stay valid JSON: {e}"));
    }

    #[test]
    fn url_encode_percent_encodes() {
        assert_eq!(
            render_template(
                "{{url_encode Name}}",
                json!({ "Name": "Tom & Jerry / S01?" })
            ),
            "Tom%20%26%20Jerry%20%2F%20S01%3F"
        );
    }

    /// The template supplies the quotes (`"title": "{{json_encode Name}}"`), so
    /// the helper must not add its own — that is what the plugin does.
    #[test]
    fn json_encode_escapes_without_adding_quotes() {
        assert_eq!(
            render_template(
                "{{json_encode Name}}",
                json!({ "Name": "He said \"hi\"" })
            ),
            r#"He said \"hi\""#
        );
        assert_eq!(
            render_template(
                "{{json_encode SeasonNumber}}",
                json!({ "SeasonNumber": 2 })
            ),
            "2"
        );

        // The canonical plugin idiom must produce valid JSON.
        let body = render_template(
            r#"{"title": "{{json_encode Name}}"}"#,
            json!({ "Name": "He said \"hi\"" }),
        );
        let parsed: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("{body} must be valid JSON: {e}"));
        assert_eq!(parsed["title"], json!("He said \"hi\""));
    }

    /// Bodies are JSON, not HTML: `'` and `&` must stay readable, while `"`,
    /// `\` and control characters must be escaped for the string literal the
    /// value almost always sits in.
    #[test]
    fn plain_substitution_is_escaped_for_a_json_string() {
        assert_eq!(
            render_template("{{Name}}", json!({ "Name": "Ocean's 11 & 12" })),
            "Ocean's 11 & 12"
        );
        assert_eq!(
            render_template("{{Name}}", json!({ "Name": "The \"Burbs" })),
            r#"The \"Burbs"#
        );
        assert_eq!(
            render_template("{{Name}}", json!({ "Name": r"C:\media\x" })),
            r"C:\\media\\x"
        );
        assert_eq!(
            render_template("{{Name}}", json!({ "Name": "line\nbreak" })),
            r"line\nbreak"
        );

        // A body built the usual way survives a hostile title.
        let body = render_template(
            r#"{"title": "{{Name}}"}"#,
            json!({ "Name": "The \"Burbs\\" }),
        );
        let parsed: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("{body} must be valid JSON: {e}"));
        assert_eq!(parsed["title"], json!("The \"Burbs\\"));
    }

    /// Triple braces stay the raw escape hatch, as in the plugin's stock
    /// templates — which is also why the double-brace form must escape.
    #[test]
    fn triple_braces_bypass_the_escaping() {
        assert_eq!(
            render_template("{{{Name}}}", json!({ "Name": "The \"Burbs" })),
            "The \"Burbs"
        );
        assert_eq!(
            render_template("{{{Name}}}", json!({ "Name": r"C:\media\x" })),
            r"C:\media\x"
        );
    }

    // --- the stock Discord template ---------------------------------------

    /// Data covering every variable the stock template interpolates outside a
    /// guard, with a title that is hostile to a JSON string literal.
    fn stock_discord_data(name: &str) -> Value {
        json!({
            "ServerId": "server-1",
            "ServerName": "remux",
            "ServerUrl": "https://media.example.test",
            "ItemId": "1d0b6a1e",
            "ItemType": "Movie",
            "Name": name,
            "Year": 2001,
        })
    }

    /// The stock template ships from the dashboard (a WASM crate) and is
    /// rendered by this registry (the server crate), so until it moved into the
    /// SDK *nothing anywhere* exercised the two halves together — which is how
    /// seven `{{{triple}}}` interpolations survived the switch from the
    /// plugin's HTML escaping to [`escape_json_string`].
    ///
    /// A triple brace bypasses the escape function, so a title carrying `"` or
    /// `\` renders a body Discord answers 400 to. That is classified `Fatal`,
    /// so there is no retry and the operator sees nothing.
    #[test]
    fn the_stock_discord_template_survives_a_title_that_is_hostile_to_json() {
        let name = r#"Ocean's "11" \ Redux"#;
        let body = render_template(
            remux_sdks::remux::DISCORD_TEMPLATE,
            stock_discord_data(name),
        );

        let parsed: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
            panic!("the stock template must render valid JSON: {e}\n{body}")
        });
        assert_eq!(
            parsed["embeds"][0]["title"],
            json!(format!("{name} (2001) has been added to remux")),
            "the title must arrive at Discord unmangled: {body}"
        );
    }

    /// …and an ordinary title must render byte-identically to what the plugin's
    /// own template produced, so the change is a fix and not a behaviour break.
    #[test]
    fn the_stock_discord_template_is_unchanged_for_an_ordinary_title() {
        let body = render_template(
            remux_sdks::remux::DISCORD_TEMPLATE,
            stock_discord_data("A Movie"),
        );
        let parsed: Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(
            parsed["embeds"][0]["title"],
            json!("A Movie (2001) has been added to remux")
        );
        assert_eq!(
            parsed["embeds"][0]["thumbnail"]["url"],
            json!("https://media.example.test/Items/1d0b6a1e/Images/Primary")
        );
    }

    // --- hook flags -------------------------------------------------------

    #[test]
    fn send_all_properties_serializes_the_dictionary_and_ignores_the_template() {
        let hook = db::Webhook {
            send_all_properties: true,
            ..hook("this template must not be used")
        };
        let registry = build_registry(std::slice::from_ref(&hook));
        let body = render(&hook, &registry, &data(json!({ "Name": "A Movie" })))
            .unwrap()
            .expect("a body must be produced");

        assert!(
            !body.contains("must not be used"),
            "template must be bypassed: {body}"
        );
        let parsed: Value =
            serde_json::from_str(&body).expect("body must be valid JSON");
        assert_eq!(parsed["Name"], json!("A Movie"));
        assert!(body.contains('\n'), "pretty-printed JSON expected: {body}");
    }

    #[test]
    fn send_all_properties_includes_the_hook_fields() {
        let hook = db::Webhook {
            send_all_properties: true,
            destination: WebhookDestination::Generic {
                headers: vec![],
                fields: vec![WebhookKeyValue {
                    key: "channel".into(),
                    value: "#general".into(),
                }],
            },
            ..hook("")
        };
        let registry = build_registry(std::slice::from_ref(&hook));
        let body = render(&hook, &registry, &data(json!({ "Name": "A Movie" })))
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["channel"], json!("#general"));
    }

    #[test]
    fn generic_destination_fields_are_available_to_the_template() {
        let hook = db::Webhook {
            destination: WebhookDestination::Generic {
                headers: vec![],
                fields: vec![WebhookKeyValue {
                    key: "channel".into(),
                    value: "#general".into(),
                }],
            },
            ..hook("{{Name}} -> {{channel}}")
        };
        let registry = build_registry(std::slice::from_ref(&hook));
        let body = render(&hook, &registry, &data(json!({ "Name": "A Movie" })))
            .unwrap()
            .unwrap();
        assert_eq!(body, "A Movie -> #general");
    }

    #[test]
    fn trim_whitespace_trims_the_rendered_body() {
        let template = "\n  {{Name}}  \n";
        let untrimmed = render_template(template, json!({ "Name": "A Movie" }));
        assert_eq!(untrimmed, template.replace("{{Name}}", "A Movie"));

        let hook = db::Webhook {
            trim_whitespace: true,
            ..hook(template)
        };
        let registry = build_registry(std::slice::from_ref(&hook));
        assert_eq!(
            render(&hook, &registry, &data(json!({ "Name": "A Movie" })))
                .unwrap()
                .unwrap(),
            "A Movie"
        );
    }

    #[test]
    fn skip_empty_message_body_suppresses_a_blank_render() {
        // The value is missing, so the body renders to whitespace only.
        let hook = db::Webhook {
            skip_empty_message_body: true,
            ..hook("  {{Name}}\n")
        };
        let registry = build_registry(std::slice::from_ref(&hook));
        assert_eq!(
            render(&hook, &registry, &data(json!({}))).unwrap(),
            None,
            "an empty body must be suppressed"
        );

        // A non-empty body is still delivered.
        assert_eq!(
            render(&hook, &registry, &data(json!({ "Name": "A Movie" })))
                .unwrap()
                .unwrap()
                .trim(),
            "A Movie"
        );
    }

    #[test]
    fn an_empty_body_is_delivered_when_the_flag_is_off() {
        let hook = hook("  {{Name}}\n");
        let registry = build_registry(std::slice::from_ref(&hook));
        assert_eq!(
            render(&hook, &registry, &data(json!({}))).unwrap(),
            Some("  \n".to_string())
        );
    }

    // --- registry wiring --------------------------------------------------

    /// The whole point of the cached registry: each hook's template is compiled
    /// once, at reload time, and rendered by name afterwards.
    #[test]
    fn build_registry_precompiles_every_hook_template() {
        let first = db::Webhook {
            id: Uuid::from_u128(1),
            ..hook("first: {{Name}}")
        };
        let second = db::Webhook {
            id: Uuid::from_u128(2),
            ..hook("second: {{Name}}")
        };
        let registry = build_registry(&[first.clone(), second.clone()]);

        assert!(
            registry.has_template(
                &first
                    .id
                    .to_string()
            ),
            "templates must be registered under the hook id"
        );
        assert_eq!(
            render(&first, &registry, &data(json!({ "Name": "X" })))
                .unwrap()
                .unwrap(),
            "first: X"
        );
        assert_eq!(
            render(&second, &registry, &data(json!({ "Name": "X" })))
                .unwrap()
                .unwrap(),
            "second: X"
        );
    }

    #[test]
    fn fresh_registry_carries_the_custom_helpers() {
        let registry = fresh_registry();
        // Rendering exercises every helper: an unregistered one is a render error.
        let body = registry
            .render_template(
                "{{#if_equals A \"a\"}}1{{/if_equals}}\
                 {{#if_exist A}}2{{/if_exist}}\
                 {{link_to A A}}{{url_encode A}}{{json_encode A}}",
                &json!({ "A": "a" }),
            )
            .expect("every custom helper must be registered on a fresh registry");
        assert_eq!(body, "12<a href='a'>a</a>aa");
    }

    #[test]
    fn a_broken_template_is_an_error_not_a_panic() {
        let hook = hook("{{#if_equals}}oops{{/if_equals}}");
        let registry = build_registry(std::slice::from_ref(&hook));
        assert!(
            render(&hook, &registry, &data(json!({}))).is_err(),
            "a helper called without its parameters must surface as an error"
        );
    }
}
