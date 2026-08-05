//! STUB — filled in by task 5 (HTTP delivery).
//!
//! A plain JSON POST: no custom headers, no destination-specific envelope, no
//! timeout or retry policy yet.

use crate::db;
use tracing::{debug, warn};

/// TODO(task 5): destination headers/envelope, timeout, retries, test support.
pub(crate) async fn deliver(hook: db::Webhook, body: String) {
    let result = reqwest::Client::new()
        .post(&hook.url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await;
    match result {
        Ok(response) => {
            debug!(webhook = %hook.name, status = %response.status().as_u16(), "webhook delivered")
        }
        Err(e) => warn!(webhook = %hook.name, error = %e, "webhook delivery failed"),
    }
}
