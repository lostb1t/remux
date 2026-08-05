//! STUB — filled in by task 4 (Handlebars rendering).
//!
//! Renders the raw template with no helpers, no whitespace trimming and no
//! `skip_empty_message_body` handling (which is what `Ok(None)` will mean).

use crate::db;
use handlebars::Handlebars;
use serde_json::{Map, Value};

/// TODO(task 4): register helpers, honour `trim_whitespace` and
/// `skip_empty_message_body`, and shape the body per destination.
pub(crate) fn render(
    hook: &db::Webhook,
    registry: &Handlebars<'static>,
    data: &Map<String, Value>,
) -> anyhow::Result<Option<String>> {
    Ok(Some(registry.render_template(&hook.template, data)?))
}
