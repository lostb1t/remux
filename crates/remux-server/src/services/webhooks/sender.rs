//! HTTP delivery of a rendered webhook body.
//!
//! Everything that decides *what* goes on the wire lives in pure functions
//! ([`shape_request`], [`detect_content_type`], [`classify_status`],
//! [`parse_retry_after`]); [`attempt_once`] only performs the POST. A new
//! destination is a new `WebhookDestination` variant plus an arm in
//! [`shape_request`].
//!
//! The rendered body is never rewrapped here. Destination-specific *content* —
//! the Discord payload, a Generic hook's extra fields — is produced by the
//! template, from the variables [`super::payload::with_hook_fields`] puts in
//! scope; that is how the Jellyfin webhook plugin works, and it is what lets a
//! template written for the plugin render verbatim.
//!
//! Delivery is fire-and-forget: [`spawn_delivery`] never blocks its caller and
//! every error is logged and swallowed, so a broken endpoint can neither stall
//! the dispatcher nor surface anywhere in the server.
//!
//! **A webhook URL is a credential.** Discord's is
//! `https://discord.com/api/webhooks/{id}/{token}` and that token is the entire
//! authentication — anyone holding it can post as the webhook indefinitely. No
//! log line in this module may contain a URL path or query; see [`redact_url`].

use crate::db;
use remux_sdks::remux::{WebhookDestination, WebhookTestResult};
use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, warn};
use uuid::Uuid;

/// Per-request timeout. Without one an endpoint that accepts the connection and
/// then blackholes it would hold its task — and its delivery slot — forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on deliveries in flight **per hook**.
const MAX_CONCURRENT_DELIVERIES_PER_HOOK: usize = 4;

/// Upper bound on a `Retry-After` we will obey. The value is remote input and
/// the waiter holds a delivery slot while it sleeps, so an endpoint must not be
/// able to pin one indefinitely.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// `Encoding.UTF8` on the plugin's `StringContent` puts the charset on the
/// header; these are the two defaults [`detect_content_type`] picks between.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// At most this many bytes of a failed response body make it into the log line.
const MAX_LOGGED_RESPONSE: usize = 512;

/// One client for the whole process: a client per delivery would rebuild the
/// TLS config and throw away the connection pool on every event.
static WEBHOOK_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("failed to build the webhook HTTP client")
});

static DELIVERY_SLOTS: LazyLock<DeliverySlots> =
    LazyLock::new(|| DeliverySlots::new(MAX_CONCURRENT_DELIVERIES_PER_HOOK));

// --- concurrency ------------------------------------------------------------

/// Delivery slots, counted **per hook**.
///
/// A single process-wide pool would let one blackholing endpoint hold every
/// slot for its full retry window — three attempts of up to 30 s each, plus
/// backoff — after which deliveries to every *healthy* hook are dropped too.
/// Keying by hook id keeps a broken Discord URL from disabling an operator's
/// working Slack and Gotify hooks; the total is still bounded, at
/// `enabled hooks × limit`.
///
/// TODO: entries are never removed, so a delete-and-recreate cycle leaves the
/// old hook's semaphore behind forever. It is tens of bytes per entry and only
/// an operator can create one, so it is not worth a mechanism today; when it
/// is, `WebhookService::reload` in `mod.rs` already knows the live hook set and
/// is the natural place to prune from.
pub(crate) struct DeliverySlots {
    limit: usize,
    per_hook: Mutex<HashMap<Uuid, Arc<Semaphore>>>,
}

impl DeliverySlots {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            per_hook: Mutex::new(HashMap::new()),
        }
    }

    /// A slot for `hook_id`, or `None` when that hook already has `limit`
    /// deliveries in flight. Never blocks and never waits.
    pub(crate) fn try_acquire(&self, hook_id: Uuid) -> Option<OwnedSemaphorePermit> {
        let semaphore = {
            // Short, await-free critical section. A poisoned lock is recovered
            // rather than propagated: a panic elsewhere must not disable
            // webhooks for the rest of the process.
            let mut per_hook = self
                .per_hook
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            per_hook
                .entry(hook_id)
                .or_insert_with(|| Arc::new(Semaphore::new(self.limit)))
                .clone()
        };
        semaphore
            .try_acquire_owned()
            .ok()
    }
}

/// Hand a rendered body to the delivery pool.
///
/// Returns immediately. When the hook already has its share of deliveries in
/// flight the event is dropped rather than queued: an unbounded backlog behind
/// a dead endpoint is worse than a missed notification, and the dispatcher must
/// never wait here.
pub(crate) fn spawn_delivery(hook: db::Webhook, body: String) {
    spawn_delivery_with(&DELIVERY_SLOTS, hook, body, DeliveryPolicy::default());
}

/// [`spawn_delivery`] with its collaborators injected, and the accept/drop
/// decision returned so both branches are observable.
///
/// The permit is taken **before** the spawn, on purpose: acquiring it inside
/// the task would bound concurrent sockets but let tasks pile up parked on the
/// semaphore — the same unbounded growth in a different allocation.
pub(crate) fn spawn_delivery_with(
    slots: &DeliverySlots,
    hook: db::Webhook,
    body: String,
    policy: DeliveryPolicy,
) -> bool {
    let Some(permit) = slots.try_acquire(hook.id) else {
        warn!(
            webhook = %hook.name,
            webhook_id = %hook.id,
            limit = slots.limit,
            "webhook already has its share of deliveries in flight, dropping event"
        );
        return false;
    };
    tokio::spawn(async move {
        // Held for the whole delivery, retries included: the slot is the
        // ceiling on work owed to one endpoint, not on one HTTP round-trip.
        let _permit = permit;
        deliver_logged(hook, body, policy).await;
    });
    true
}

// --- delivery ---------------------------------------------------------------

/// How hard a single delivery tries. Extracted so tests can shrink the backoff.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeliveryPolicy {
    pub attempts: u32,
    /// Base delay in milliseconds, grown exponentially with jitter.
    pub retry_delay_ms: u64,
}

impl Default for DeliveryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            retry_delay_ms: 500,
        }
    }
}

/// Deliver `body` to `hook`, retrying transient failures. Never fails: a broken
/// webhook is a log line, nothing more.
pub(crate) async fn deliver(hook: db::Webhook, body: String) {
    deliver_logged(hook, body, DeliveryPolicy::default()).await;
}

async fn deliver_logged(hook: db::Webhook, body: String, policy: DeliveryPolicy) {
    if let Err(e) = deliver_with(&hook, &body, &policy).await {
        // No URL path, ever: it is the webhook's credential.
        warn!(
            webhook = %hook.name,
            webhook_id = %hook.id,
            endpoint = %redact_url(&hook.url),
            error = %e,
            "webhook delivery failed, giving up"
        );
    }
}

/// The retried delivery, with its outcome still visible. [`deliver`] is this
/// plus the logging.
///
/// Only *transient* failures are retried. Hand-rolled rather than built on
/// `remux_utils::retry!` because that macro retries every error
/// unconditionally, which would spend three attempts on a 401 and — worse for
/// Discord — hammer a 429 on a fixed backoff while ignoring the `Retry-After`
/// the endpoint just sent, escalating the very rate limit it is reacting to.
pub(crate) async fn deliver_with(
    hook: &db::Webhook,
    body: &str,
    policy: &DeliveryPolicy,
) -> anyhow::Result<()> {
    let attempts = policy
        .attempts
        .max(1);
    let mut last: Option<SendError> = None;
    for attempt in 0..attempts {
        match attempt_once(hook, body).await {
            Ok(response) => {
                debug!(
                    webhook = %hook.name,
                    webhook_id = %hook.id,
                    status = %response.status().as_u16(),
                    "webhook delivered"
                );
                return Ok(());
            }
            // Nothing about a second identical request would change the answer.
            Err(e) if e.retryability == Retryability::Fatal => {
                return Err(e.into());
            }
            Err(e) => {
                if attempt + 1 < attempts {
                    // The endpoint's own instruction wins over our backoff.
                    let wait = e
                        .retry_after
                        .unwrap_or_else(|| backoff(policy.retry_delay_ms, attempt));
                    tokio::time::sleep(wait).await;
                }
                last = Some(e);
            }
        }
    }
    Err(last
        .expect("at least one attempt is always made")
        .into())
}

/// A single POST, for callers that want one attempt and no retry policy.
pub(crate) async fn send_once(
    hook: &db::Webhook,
    body: &str,
) -> anyhow::Result<reqwest::Response> {
    attempt_once(hook, body)
        .await
        .map_err(anyhow::Error::from)
}

/// One POST, reported as the admin API's "test this webhook" result.
///
/// A single attempt on purpose: the retry policy exists so a transient failure
/// does not lose a *notification*, but here an operator is waiting on the
/// answer and what they need to see is what the endpoint said just now. The
/// process-wide [`REQUEST_TIMEOUT`] still applies, so a hostile URL cannot pin
/// the request handler.
///
/// The error text comes from [`SendError`], which is already redacted — a
/// webhook URL is a credential and must not travel back to the browser.
pub(crate) async fn send_test(hook: &db::Webhook, body: &str) -> WebhookTestResult {
    match attempt_once(hook, body).await {
        Ok(response) => WebhookTestResult {
            success: true,
            status_code: Some(
                response
                    .status()
                    .as_u16(),
            ),
            error: None,
        },
        Err(e) => WebhookTestResult {
            success: false,
            status_code: e
                .status
                .map(|status| status.as_u16()),
            error: Some(e.to_string()),
        },
    }
}

/// One POST, classified.
///
/// `reqwest` treats a 4xx/5xx as a perfectly good response, so the status is
/// checked here: without this every failed delivery would be reported as a
/// success and the retry would never fire.
async fn attempt_once(
    hook: &db::Webhook,
    body: &str,
) -> Result<reqwest::Response, SendError> {
    let shaped = shape_request(hook, body);
    let mut request = WEBHOOK_CLIENT
        .post(&hook.url)
        .header(CONTENT_TYPE, shaped.content_type);
    for (name, value) in shaped.headers {
        request = request.header(name, value);
    }
    let response = request
        .body(shaped.body)
        .send()
        .await
        .map_err(|e| SendError {
            // DNS, connect and timeout failures are exactly what a retry is
            // for; a URL that does not parse fails identically every time.
            retryability: if e.is_builder() {
                Retryability::Fatal
            } else {
                Retryability::Transient
            },
            retry_after: None,
            // Nothing reached the endpoint, so there is no status to report.
            status: None,
            // `reqwest`'s Display includes the URL, which is the credential.
            message: format!("request failed: {}", redact_reqwest_error(&e)),
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retryability = classify_status(status);
    let retry_after = retry_after(status, response.headers());
    let detail = response
        .text()
        .await
        .unwrap_or_default();
    Err(SendError {
        retryability,
        retry_after,
        status: Some(status),
        message: format!(
            "endpoint returned {status}: {}",
            truncate(detail.trim(), MAX_LOGGED_RESPONSE)
        ),
    })
}

/// Whether a failed attempt is worth repeating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Retryability {
    Transient,
    Fatal,
}

/// A failed attempt, plus what the caller should do about it.
#[derive(Debug)]
pub(crate) struct SendError {
    pub retryability: Retryability,
    /// The endpoint's own instruction, when it sent one.
    pub retry_after: Option<Duration>,
    /// The status the endpoint answered with, or `None` when the request never
    /// got that far. Reported by [`send_test`]; the retry loop only cares about
    /// [`Retryability`].
    pub status: Option<StatusCode>,
    message: String,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SendError {}

/// Only failures a later attempt could plausibly survive are retried: 5xx,
/// `408 Request Timeout` and `429 Too Many Requests`. A 400/401/403/404 is the
/// endpoint telling us the request itself is wrong — repeating it verbatim
/// wastes attempts and, on Discord, counts against the rate limit.
pub(crate) fn classify_status(status: StatusCode) -> Retryability {
    if status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
        )
    {
        Retryability::Transient
    } else {
        Retryability::Fatal
    }
}

/// How long the endpoint asked us to wait, for a rate limit only.
///
/// `Retry-After` first, then Discord's `X-RateLimit-Reset-After`. Restricted to
/// 429 on purpose: Discord attaches its rate-limit headers to responses
/// generally, so honouring them on a 5xx would let a `x-ratelimit-reset-after:
/// 0` collapse the exponential backoff into three immediate retries against an
/// endpoint that is already struggling.
fn retry_after(status: StatusCode, headers: &HeaderMap) -> Option<Duration> {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    ["retry-after", "x-ratelimit-reset-after"]
        .into_iter()
        .filter_map(|name| {
            headers
                .get(name)?
                .to_str()
                .ok()
        })
        .find_map(parse_retry_after)
}

/// `Retry-After` as a delay, capped at [`MAX_RETRY_AFTER`].
///
/// Only the delta-seconds form is understood — that is what Discord sends, and
/// it may be fractional. An HTTP-date, or anything unparseable, yields `None`
/// and the normal backoff applies.
///
/// The cap is applied to the `f64` **before** the conversion:
/// `Duration::from_secs_f64` panics outside `Duration`'s range, and this value
/// comes straight off a remote response header — `Retry-After: 1e30` must be a
/// clamped wait, not a panic in the delivery task.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds: f64 = value
        .trim()
        .parse()
        .ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(
        seconds.min(MAX_RETRY_AFTER.as_secs_f64()),
    ))
}

/// `base * 2^attempt` plus jitter in `[0, base/2)`, mirroring
/// `remux_utils::retry!`.
fn backoff(base_ms: u64, attempt: u32) -> Duration {
    let exponential = base_ms.saturating_mul(1u64 << attempt.min(10));
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos() as u64 % (base_ms / 2 + 1))
        .unwrap_or(0);
    Duration::from_millis(exponential.saturating_add(jitter))
}

// --- request shaping --------------------------------------------------------

/// Everything a destination decides about the request, resolved without I/O.
pub(crate) struct ShapedRequest {
    pub body: String,
    pub content_type: HeaderValue,
    /// Extra headers, `Content-Type` excluded — it belongs on the content.
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

/// Turn a rendered body into the request `hook`'s destination expects.
pub(crate) fn shape_request(hook: &db::Webhook, rendered: &str) -> ShapedRequest {
    match &hook.destination {
        // The rendered body goes out verbatim; the operator's headers are
        // applied on top, with `Content-Type` pulled out because it describes
        // the content rather than being a header of its own.
        WebhookDestination::Generic { headers, .. } => {
            let mut content_type =
                HeaderValue::from_static(detect_content_type(rendered));
            let mut extra = Vec::with_capacity(headers.len());
            for pair in headers {
                let (key, value) = (
                    pair.key
                        .as_str(),
                    pair.value
                        .as_str(),
                );
                if key.is_empty() || value.is_empty() {
                    continue;
                }
                if key.eq_ignore_ascii_case(CONTENT_TYPE.as_str()) {
                    match HeaderValue::from_str(value) {
                        Ok(value) => content_type = value,
                        Err(_) => warn!(
                            webhook = %hook.name,
                            "invalid Content-Type on webhook, using the detected one"
                        ),
                    }
                    continue;
                }
                match (
                    HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    (Ok(name), Ok(value)) => extra.push((name, value)),
                    // The name is operator-chosen and safe to log; the value
                    // may be a token and is not.
                    _ => {
                        warn!(webhook = %hook.name, header = %key, "skipping invalid webhook header")
                    }
                }
            }
            ShapedRequest {
                body: rendered.to_string(),
                content_type,
                headers: extra,
            }
        }
        // Same as the plugin's `DiscordClient`: the template already rendered
        // the whole Discord payload — post it as-is, as JSON, with no headers
        // of its own. The destination's settings reached the template through
        // `payload::with_hook_fields`, not through this function.
        WebhookDestination::Discord { .. } => ShapedRequest {
            body: rendered.to_string(),
            content_type: HeaderValue::from_static(JSON_CONTENT_TYPE),
            headers: Vec::new(),
        },
    }
}

/// The content type a rendered body should be sent as when the operator has not
/// named one: templates that produce JSON are the common case, but a template
/// is free to produce anything.
pub(crate) fn detect_content_type(body: &str) -> &'static str {
    if serde_json::from_str::<Value>(body).is_ok() {
        JSON_CONTENT_TYPE
    } else {
        TEXT_CONTENT_TYPE
    }
}

// --- redaction --------------------------------------------------------------

/// Scheme and host only.
///
/// A webhook URL's path is a credential: Discord's is
/// `https://discord.com/api/webhooks/{id}/{token}`, and that token is the whole
/// authentication. Log excerpts end up in bug reports, so nothing past the host
/// may appear in one.
pub(crate) fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => format!("{}://{host}", parsed.scheme()),
            None => parsed
                .scheme()
                .to_string(),
        },
        Err(_) => "<unparseable url>".to_string(),
    }
}

/// `reqwest::Error`'s `Display` embeds the request URL, so it is stripped
/// before the message reaches a log line.
fn redact_reqwest_error(error: &reqwest::Error) -> String {
    match error.url() {
        Some(url) => error
            .to_string()
            .replace(url.as_str(), &redact_url(url.as_str())),
        None => error.to_string(),
    }
}

/// Truncate on a char boundary — response bodies are arbitrary bytes.
fn truncate(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use remux_sdks::remux::{
        DiscordMentionType, NotificationType, WebhookDestination, WebhookItemTypes,
        WebhookKeyValue,
    };
    use std::time::Instant;

    /// Short enough that the suite does not crawl, long enough that the two
    /// backoff sleeps are observable.
    const FAST: DeliveryPolicy = DeliveryPolicy {
        attempts: 3,
        retry_delay_ms: 20,
    };

    fn hook(url: &str, destination: WebhookDestination) -> db::Webhook {
        let now = chrono::Utc::now();
        db::Webhook {
            id: Uuid::from_u128(100),
            name: "test".into(),
            enabled: true,
            url: url.into(),
            template: "{{Name}}".into(),
            destination,
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

    fn generic(url: &str, headers: &[(&str, &str)]) -> db::Webhook {
        hook(
            url,
            WebhookDestination::Generic {
                headers: headers
                    .iter()
                    .map(|(key, value)| WebhookKeyValue {
                        key: (*key).into(),
                        value: (*value).into(),
                    })
                    .collect(),
                fields: vec![],
            },
        )
    }

    fn discord_hook(url: &str, mention_type: DiscordMentionType) -> db::Webhook {
        hook(
            url,
            WebhookDestination::Discord {
                avatar_url: None,
                bot_username: None,
                embed_color: None,
                mention_type,
            },
        )
    }

    fn content_type(hook: &db::Webhook, body: &str) -> String {
        shape_request(hook, body)
            .content_type
            .to_str()
            .expect("content type must be a valid header value")
            .to_string()
    }

    /// Poll `condition` until it holds, failing the test rather than hanging.
    async fn eventually(what: &str, mut condition: impl AsyncFnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition().await {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    // --- redaction --------------------------------------------------------

    /// A Discord webhook token is the entire credential — it must never reach a
    /// log line, and log lines are what operators paste into issue trackers.
    #[test]
    fn redact_url_keeps_only_the_scheme_and_host() {
        let secret = "https://discord.com/api/webhooks/123456789/aVerySecretToken";
        let redacted = redact_url(secret);
        assert_eq!(redacted, "https://discord.com");
        assert!(
            !redacted.contains("aVerySecretToken"),
            "the token must not survive redaction: {redacted}"
        );
        assert!(!redacted.contains("123456789"));

        // Query strings are credentials too (Slack, Gotify, Teams).
        assert_eq!(
            redact_url("https://hooks.example.test/services/T/B/xyz?token=abc"),
            "https://hooks.example.test"
        );
        // Operator input may not parse at all.
        assert_eq!(redact_url("not a url"), "<unparseable url>");
    }

    /// The transport error's own `Display` embeds the URL; the message we log
    /// must not.
    #[tokio::test]
    async fn a_transport_error_message_carries_no_url_path() {
        let hook = generic("http://127.0.0.1:1/api/webhooks/123/secret-token", &[]);
        let error = send_once(&hook, "ping")
            .await
            .expect_err("nothing is listening on port 1");
        let message = error.to_string();
        assert!(
            !message.contains("secret-token"),
            "the URL path leaked into the error: {message}"
        );
    }

    // --- detect_content_type ----------------------------------------------

    #[test]
    fn detect_content_type_recognises_json() {
        assert!(detect_content_type(r#"{"a": 1}"#).starts_with("application/json"));
        assert!(detect_content_type("[1, 2]").starts_with("application/json"));
    }

    #[test]
    fn detect_content_type_falls_back_to_text() {
        assert!(detect_content_type("a plain line").starts_with("text/plain"));
        assert!(detect_content_type("{not json").starts_with("text/plain"));
        assert!(detect_content_type("").starts_with("text/plain"));
    }

    // --- shape_request: generic -------------------------------------------

    #[test]
    fn generic_sends_the_rendered_body_verbatim_with_a_detected_content_type() {
        let hook = generic("https://example.test/hook", &[]);
        let shaped = shape_request(&hook, "hello");
        assert_eq!(shaped.body, "hello");
        assert!(
            content_type(&hook, "hello").starts_with("text/plain"),
            "a non-JSON body must be sent as text"
        );
        assert!(content_type(&hook, r#"{"a":1}"#).starts_with("application/json"));
    }

    #[test]
    fn generic_content_type_header_overrides_the_detected_one() {
        let hook = generic(
            "https://example.test/hook",
            &[("content-type", "application/x-www-form-urlencoded")],
        );
        assert_eq!(
            content_type(&hook, r#"{"a":1}"#),
            "application/x-www-form-urlencoded",
            "the operator's Content-Type wins, case-insensitively"
        );
        assert!(
            shape_request(&hook, "x")
                .headers
                .is_empty(),
            "Content-Type belongs on the content, not the header map"
        );
    }

    #[test]
    fn generic_applies_the_operator_headers() {
        let hook = generic(
            "https://example.test/hook",
            &[("X-Token", "s3cret"), ("X-Other", "v")],
        );
        let names: Vec<String> = shape_request(&hook, "x")
            .headers
            .iter()
            .map(|(name, _)| {
                name.as_str()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["x-token", "x-other"]);
    }

    /// Header names and values are operator input: a malformed pair must be
    /// dropped, never panic.
    #[test]
    fn generic_skips_empty_and_malformed_headers() {
        let hook = generic(
            "https://example.test/hook",
            &[
                ("", "no key"),
                ("X-No-Value", ""),
                ("Bad Name", "v"),
                ("X-Bad-Value", "line\nbreak"),
                ("X-Good", "v"),
            ],
        );
        let shaped = shape_request(&hook, "x");
        assert_eq!(
            shaped
                .headers
                .len(),
            1
        );
        assert_eq!(
            shaped.headers[0]
                .0
                .as_str(),
            "x-good"
        );
    }

    #[test]
    fn a_malformed_operator_content_type_falls_back_to_the_detected_one() {
        let hook = generic("https://example.test/hook", &[("Content-Type", "a\nb")]);
        assert!(content_type(&hook, "plain").starts_with("text/plain"));
    }

    // --- shape_request: discord -------------------------------------------

    /// Parity with the plugin's `DiscordClient`: the template renders the whole
    /// Discord payload, so the sender must post it byte for byte. Wrapping it
    /// in a server-built envelope would break every template copied from the
    /// plugin.
    #[test]
    fn discord_posts_the_rendered_body_unmodified() {
        let rendered = r#"{"content": "@everyone", "embeds": [{"title": "A Movie"}]}"#;
        let hook =
            discord_hook("https://example.test/hook", DiscordMentionType::Everyone);
        let shaped = shape_request(&hook, rendered);
        assert_eq!(shaped.body, rendered, "the body must not be rewrapped");
        assert!(
            content_type(&hook, rendered).starts_with("application/json"),
            "Discord always takes JSON, whatever the body looks like"
        );
        assert!(
            shaped
                .headers
                .is_empty(),
            "the plugin sends no custom headers to Discord"
        );

        // Even a body that is not valid JSON goes out untouched, as JSON: the
        // template — not the sender — owns the payload.
        let broken = shape_request(&hook, "not json at all");
        assert_eq!(broken.body, "not json at all");
        assert!(
            broken
                .content_type
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
    }

    // --- the actual wire request ------------------------------------------

    /// `shape_request` deciding something is worthless if the decision never
    /// reaches the socket. This matches on the received bytes, so deleting the
    /// header loop in `attempt_once` — silently dropping an operator's auth
    /// token from every delivery — fails here.
    #[tokio::test]
    async fn the_posted_request_carries_the_body_headers_and_content_type() {
        let server = MockServer::start_async().await;
        let body = r#"{"text":"A Movie & \"friends\""}"#;
        let mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/hook")
                    .header("x-auth-token", "s3cret")
                    .header("x-other", "v")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(body);
                then.status(200);
            })
            .await;

        let hook = generic(
            &server.url("/hook"),
            &[("X-Auth-Token", "s3cret"), ("X-Other", "v")],
        );
        send_once(&hook, body)
            .await
            .expect("the request must match the mock exactly");
        mock.assert_hits_async(1)
            .await;
    }

    /// The operator's `Content-Type` must reach the wire too, not just
    /// `ShapedRequest`.
    #[tokio::test]
    async fn the_operator_content_type_reaches_the_wire() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body("a=1&b=2");
                then.status(200);
            })
            .await;

        let hook = generic(
            &server.url("/hook"),
            &[("Content-Type", "application/x-www-form-urlencoded")],
        );
        send_once(&hook, "a=1&b=2")
            .await
            .expect("the operator's content type must be the one sent");
        mock.assert_hits_async(1)
            .await;
    }

    /// Discord gets the rendered bytes and nothing else.
    #[tokio::test]
    async fn a_discord_delivery_posts_the_template_output_byte_for_byte() {
        let server = MockServer::start_async().await;
        let rendered =
            "{\n    \"content\": \"@here\",\n    \"embeds\": [{\"color\": 3381759}]\n}";
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(rendered);
                then.status(204);
            })
            .await;

        let hook = discord_hook(&server.url("/hook"), DiscordMentionType::Here);
        send_once(&hook, rendered)
            .await
            .expect("the rendered payload must be posted unchanged");
        mock.assert_hits_async(1)
            .await;
    }

    // --- status classification --------------------------------------------

    #[test]
    fn only_recoverable_statuses_are_retried() {
        for status in [500u16, 502, 503, 504, 408, 429] {
            assert_eq!(
                classify_status(StatusCode::from_u16(status).unwrap()),
                Retryability::Transient,
                "{status} must be retried"
            );
        }
        for status in [400u16, 401, 403, 404, 405, 410, 422] {
            assert_eq!(
                classify_status(StatusCode::from_u16(status).unwrap()),
                Retryability::Fatal,
                "{status} must not be retried"
            );
        }
    }

    #[test]
    fn parse_retry_after_reads_delta_seconds() {
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(
            parse_retry_after(" 0.25 "),
            Some(Duration::from_millis(250))
        );
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
    }

    /// The value is remote input: an HTTP-date, junk, or a hostile number must
    /// not pin a delivery slot.
    #[test]
    fn parse_retry_after_rejects_what_it_cannot_trust() {
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("NaN"), None);
        assert_eq!(parse_retry_after("999999"), Some(MAX_RETRY_AFTER));
    }

    /// `Duration::from_secs_f64` panics outside `Duration`'s range, so the cap
    /// has to be applied to the `f64` before the conversion. These all parse as
    /// finite, positive floats and would otherwise panic the delivery task —
    /// remotely, from a response header, on any failing status.
    #[test]
    fn parse_retry_after_clamps_instead_of_panicking_on_huge_values() {
        for value in [
            "1e30",
            "99999999999999999999999",
            "179769313486231570000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            &f64::MAX.to_string(),
        ] {
            assert_eq!(
                parse_retry_after(value),
                Some(MAX_RETRY_AFTER),
                "{value} must clamp to the cap, not panic"
            );
        }
    }

    /// Discord attaches rate-limit headers to responses generally. Obeying them
    /// on a 5xx would turn three spaced attempts into an immediate burst
    /// against an endpoint that is already failing.
    #[test]
    fn rate_limit_headers_are_only_honoured_on_a_429() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-reset-after", HeaderValue::from_static("0"));
        headers.insert("retry-after", HeaderValue::from_static("0"));

        assert_eq!(
            retry_after(StatusCode::TOO_MANY_REQUESTS, &headers),
            Some(Duration::ZERO),
            "a 429 is exactly what these headers are for"
        );
        for status in [500u16, 502, 503, 408] {
            assert_eq!(
                retry_after(StatusCode::from_u16(status).unwrap(), &headers),
                None,
                "{status} must fall back to the exponential backoff"
            );
        }
    }

    // --- send_once: status handling ---------------------------------------

    #[tokio::test]
    async fn send_once_reports_a_non_2xx_as_an_error() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(500)
                    .body("boom");
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let error = send_once(&hook, "ping")
            .await
            .expect_err("a 500 must not be reported as a success");
        let message = error.to_string();
        assert!(
            message.contains("500"),
            "the error must name the status: {message}"
        );
        mock.assert_hits_async(1)
            .await;
    }

    #[tokio::test]
    async fn send_once_accepts_any_2xx() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(204);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        assert!(
            send_once(&hook, "ping")
                .await
                .is_ok()
        );
    }

    // --- retry -------------------------------------------------------------

    #[tokio::test]
    async fn delivery_retries_until_the_attempt_budget_is_spent() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(500);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        assert!(
            deliver_with(&hook, "ping", &FAST)
                .await
                .is_err()
        );
        assert_eq!(
            mock.hits_async()
                .await,
            3,
            "a persistent 5xx must be retried up to the attempt budget"
        );
    }

    /// A 400 means the request itself is wrong: repeating it verbatim cannot
    /// help, and on Discord it burns rate limit.
    #[tokio::test]
    async fn a_fatal_status_is_attempted_exactly_once() {
        for status in [400u16, 401, 403, 404] {
            let server = MockServer::start_async().await;
            let mock = server
                .mock_async(move |when, then| {
                    when.path("/hook");
                    then.status(status);
                })
                .await;

            let hook = generic(&server.url("/hook"), &[]);
            assert!(
                deliver_with(&hook, "ping", &FAST)
                    .await
                    .is_err(),
                "{status} must still be reported as a failure"
            );
            assert_eq!(
                mock.hits_async()
                    .await,
                1,
                "{status} must not be retried"
            );
        }
    }

    /// A 429 *is* retried — and on the endpoint's own schedule.
    #[tokio::test]
    async fn a_rate_limit_is_retried_on_the_endpoint_schedule() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(429)
                    .header("retry-after", "0.05");
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        // A base delay far larger than the endpoint's instruction: if
        // `Retry-After` were ignored, this would take ~30 s instead of ~0.1 s.
        let policy = DeliveryPolicy {
            attempts: 3,
            retry_delay_ms: 10_000,
        };
        let started = Instant::now();
        assert!(
            deliver_with(&hook, "ping", &policy)
                .await
                .is_err()
        );
        assert_eq!(
            mock.hits_async()
                .await,
            3,
            "a 429 must be retried"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Retry-After must override the backoff, took {:?}",
            started.elapsed()
        );
    }

    /// The same headers on a 5xx must be ignored: the exponential schedule has
    /// to survive an endpoint that advertises a zero rate-limit reset while it
    /// is failing for an unrelated reason.
    #[tokio::test]
    async fn a_server_error_keeps_the_exponential_schedule() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(500)
                    .header("x-ratelimit-reset-after", "0")
                    .header("retry-after", "0");
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let policy = DeliveryPolicy {
            attempts: 3,
            retry_delay_ms: 150,
        };
        let started = Instant::now();
        assert!(
            deliver_with(&hook, "ping", &policy)
                .await
                .is_err()
        );
        assert_eq!(
            mock.hits_async()
                .await,
            3
        );
        // Two backoff sleeps of at least 150 ms and 300 ms. Honouring the
        // headers would collapse this to a burst of three immediate requests.
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "the backoff was skipped, took only {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn delivery_stops_at_the_first_success() {
        let server = MockServer::start_async().await;
        let failing = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(500);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let policy = DeliveryPolicy {
            attempts: 3,
            retry_delay_ms: 100,
        };
        let task =
            tokio::spawn(async move { deliver_with(&hook, "ping", &policy).await });

        // Let the first two attempts fail, then make the endpoint healthy again
        // while the last backoff sleep is still running.
        eventually("two failed attempts", async || {
            failing
                .hits_async()
                .await
                >= 2
        })
        .await;
        let healthy = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(200);
            })
            .await;
        let failed_attempts = failing
            .hits_async()
            .await;
        failing
            .delete_async()
            .await;

        task.await
            .expect("the delivery task must not panic")
            .expect("the third attempt succeeded, so the delivery must succeed");
        assert_eq!(failed_attempts, 2);
        assert_eq!(
            healthy
                .hits_async()
                .await,
            1,
            "the retry must stop at the first success"
        );
    }

    /// A failing webhook must never propagate: `deliver` logs and swallows.
    #[tokio::test]
    async fn deliver_swallows_every_error() {
        // Nothing is listening on this port, so every attempt fails at connect.
        let hook = generic("http://127.0.0.1:1/hook", &[]);
        deliver_with(&hook, "ping", &FAST)
            .await
            .expect_err("a connection failure must surface as an error internally");
        // …but the fire-and-forget entry point returns quietly.
        deliver(hook, "ping".into()).await;
    }

    #[tokio::test]
    async fn an_unparseable_url_is_an_error_not_a_panic() {
        let hook = generic("not a url", &[]);
        assert!(
            send_once(&hook, "ping")
                .await
                .is_err()
        );
    }

    // --- delivery slots ----------------------------------------------------

    /// The point of keying by hook: a saturated endpoint must not consume the
    /// slots of a healthy one.
    #[test]
    fn slots_are_counted_per_hook() {
        let slots = DeliverySlots::new(2);
        let busy = Uuid::from_u128(1);
        let healthy = Uuid::from_u128(2);

        let first = slots
            .try_acquire(busy)
            .expect("a fresh hook has slots");
        let _second = slots
            .try_acquire(busy)
            .expect("up to the limit");
        assert!(
            slots
                .try_acquire(busy)
                .is_none(),
            "past the limit a hook gets nothing"
        );
        assert!(
            slots
                .try_acquire(healthy)
                .is_some(),
            "a saturated hook must not starve another hook"
        );

        drop(first);
        assert!(
            slots
                .try_acquire(busy)
                .is_some(),
            "a released slot comes back"
        );
    }

    /// Entry condition 3: the drop branch drops, and it drops silently rather
    /// than queueing — a saturated hook must produce no request at all.
    #[tokio::test]
    async fn spawn_delivery_drops_the_event_when_the_hook_is_saturated() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(200);
            })
            .await;

        let slots = DeliverySlots::new(1);
        let hook = generic(&server.url("/hook"), &[]);
        let held = slots
            .try_acquire(hook.id)
            .expect("a fresh hook has a slot");

        assert!(
            !spawn_delivery_with(&slots, hook.clone(), "ping".into(), FAST),
            "with no slot the delivery must be dropped"
        );
        // A different hook is untouched by the first one's saturation.
        let other = db::Webhook {
            id: Uuid::from_u128(200),
            ..hook.clone()
        };
        assert!(
            spawn_delivery_with(&slots, other, "ping".into(), FAST),
            "another hook must still be delivered"
        );

        drop(held);
        assert!(
            spawn_delivery_with(&slots, hook, "ping".into(), FAST),
            "the slot is available again once the delivery finishes"
        );

        // Exactly the two accepted deliveries reached the endpoint.
        eventually("both accepted deliveries", async || {
            mock.hits_async()
                .await
                >= 2
        })
        .await;
        assert_eq!(
            mock.hits_async()
                .await,
            2,
            "the dropped delivery must not have been queued"
        );
    }

    /// The permit is taken *before* the spawn: acquiring it inside the task
    /// would bound sockets but let tasks pile up parked on the semaphore.
    /// Nothing is awaited between the call and the assertion, so the spawned
    /// task cannot have run — this observes the synchronous acquisition only.
    #[tokio::test]
    async fn spawn_delivery_takes_its_permit_before_spawning() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(200)
                    .delay(Duration::from_millis(200));
            })
            .await;

        let slots = DeliverySlots::new(1);
        let hook = generic(&server.url("/hook"), &[]);
        assert!(spawn_delivery_with(
            &slots,
            hook.clone(),
            "ping".into(),
            FAST
        ));
        assert!(
            slots
                .try_acquire(hook.id)
                .is_none(),
            "the permit must already be held before the task is polled"
        );

        // And it is held for the whole delivery, then released.
        eventually("the slot to come back", async || {
            slots
                .try_acquire(hook.id)
                .is_some()
        })
        .await;
    }
}
