//! HTTP delivery of a rendered webhook body.
//!
//! A new destination is a new `WebhookDestination` variant plus an arm in
//! [`shape_request`].
//!
//! The rendered body is never rewrapped here. Destination-specific *content* —
//! the Discord payload, a Generic hook's extra fields — is produced by the
//! template, from the variables [`super::payload::with_hook_fields`] puts in
//! scope, as in the Jellyfin webhook plugin.
//!
//! Delivery is fire-and-forget: [`spawn_delivery`] never blocks its caller and
//! every error is logged and swallowed.
//!
//! **A webhook URL is a credential.** Discord's is
//! `https://discord.com/api/webhooks/{id}/{token}`, so no log line in this
//! module may contain a URL path or query; see [`redact_url`].

use super::throttle::LogThrottle;
use crate::db;
use remux_sdks::remux::{WebhookDestination, WebhookTestResult};
use remux_utils::retry::backoff;
use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, warn};
use uuid::Uuid;

/// Per-request timeout. Without one an endpoint that accepts the connection and
/// then blackholes it would hold its task — and its delivery slot — forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for the admin "test this webhook" request, applied per request so
/// [`REQUEST_TIMEOUT`] keeps governing background delivery.
const TEST_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on deliveries in flight **per hook**.
const MAX_CONCURRENT_DELIVERIES_PER_HOOK: usize = 4;

/// Upper bound on a `Retry-After` we will obey: the value is remote input and
/// the waiter holds a delivery slot while it sleeps.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// `Encoding.UTF8` on the plugin's `StringContent` puts the charset on the
/// header; these are the two defaults [`detect_content_type`] picks between.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// At most this many bytes of a failed response body make it into the log line.
const MAX_LOGGED_RESPONSE: usize = 512;

/// One client for the whole process, so the connection pool survives.
///
/// Redirects are **not** followed: the target is chosen by the remote server at
/// request time and would be reached from inside the network this server runs
/// in. A 3xx is reported as the non-2xx it is.
static WEBHOOK_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build the webhook HTTP client")
});

static DELIVERY_SLOTS: LazyLock<DeliverySlots> =
    LazyLock::new(|| DeliverySlots::new(MAX_CONCURRENT_DELIVERIES_PER_HOOK));

/// How often a hook may repeat its "dropping event" line.
///
/// Unthrottled this is an amplification primitive: the drop branch is reachable
/// from an unauthenticated caller (`AuthenticationFailure` in `api::users`), so
/// the line rate would be the attacker's request rate.
const SATURATION_WARN_WINDOW: Duration = Duration::from_secs(60);

static SATURATION_WARNINGS: LazyLock<LogThrottle> =
    LazyLock::new(|| LogThrottle::new(SATURATION_WARN_WINDOW));

// --- concurrency ------------------------------------------------------------

/// Delivery slots, counted **per hook**.
///
/// Not a single process-wide pool: one blackholing endpoint would hold every
/// slot for its full retry window and drop deliveries to healthy hooks too. The
/// total stays bounded at `enabled hooks × limit`.
///
/// Entries are created on first delivery and dropped by [`Self::retain`], which
/// [`super::WebhookService::reload`] calls with the live hook set — otherwise a
/// deleted or disabled hook would keep its entry until restart.
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
            // Short, await-free critical section. A poisoned lock is recovered:
            // a panic elsewhere must not disable webhooks for the process.
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

    /// Forget every hook not in `live`, except one with a delivery still in
    /// flight: dropping that entry would let the next delivery build a fresh
    /// semaphore and exceed the limit. A later pass collects it.
    pub(crate) fn retain(&self, live: &HashSet<Uuid>) {
        let mut per_hook = self
            .per_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        per_hook.retain(|hook_id, semaphore| {
            live.contains(hook_id) || semaphore.available_permits() < self.limit
        });
    }
}

/// [`DeliverySlots::retain`] on the process-wide slots.
pub(crate) fn retain_delivery_slots(live: &HashSet<Uuid>) {
    DELIVERY_SLOTS.retain(live);
}

/// Hand a rendered body to the delivery pool. A hook with its share already in
/// flight has the event dropped rather than queued: the dispatcher never waits.
pub(crate) fn spawn_delivery(hook: db::Webhook, body: String) {
    spawn_delivery_with(&DELIVERY_SLOTS, hook, body, DeliveryPolicy::default());
}

/// [`spawn_delivery`] with its collaborators injected.
///
/// The permit is taken **before** the spawn, on purpose: acquiring it inside
/// the task would bound concurrent sockets but let tasks pile up parked on the
/// semaphore.
pub(crate) fn spawn_delivery_with(
    slots: &DeliverySlots,
    hook: db::Webhook,
    body: String,
    policy: DeliveryPolicy,
) -> bool {
    let Some(permit) = slots.try_acquire(hook.id) else {
        // Only the line is rate-limited, never the drop — see
        // [`SATURATION_WARN_WINDOW`].
        if let Some(dropped_since) = SATURATION_WARNINGS.allow(hook.id) {
            warn!(
                webhook = %hook.name,
                webhook_id = %hook.id,
                limit = slots.limit,
                dropped_since,
                "webhook already has its share of deliveries in flight, dropping event"
            );
        }
        return false;
    };
    tokio::spawn(async move {
        // Held for the whole delivery, retries and `Retry-After` sleeps
        // included: the slot is the ceiling on work owed to one endpoint, not on
        // one HTTP round-trip. So a rate-limiting endpoint can pin all its slots
        // for ~210s (`attempts × REQUEST_TIMEOUT + (attempts - 1) ×
        // MAX_RETRY_AFTER`) and have its new events dropped — deliberately:
        // releasing the permit around the sleep would only turn "dropped" into
        // "tasks parked on a semaphore", and keep pushing at an endpoint that
        // just asked us to stop.
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

/// The retried delivery, with its outcome still visible.
///
/// Only *transient* failures are retried. Hand-rolled rather than built on
/// `remux_utils::retry!`, which retries unconditionally and on a fixed backoff:
/// that would spend attempts on a 401 and ignore a 429's `Retry-After`.
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
/// A single attempt on purpose: an operator is waiting on the answer and what
/// they need is what the endpoint said just now.
///
/// **The remote response body never travels back to the caller.** The hook's
/// URL, headers and body are all admin-controlled and unrestricted by host, so
/// echoing it would make this endpoint a read primitive. Only the status comes
/// back; the body is logged server-side instead, under the same redaction
/// [`deliver_logged`] applies (which is never on this path).
pub(crate) async fn send_test(hook: &db::Webhook, body: &str) -> WebhookTestResult {
    send_test_with(hook, body, TEST_REQUEST_TIMEOUT).await
}

/// [`send_test`] with the timeout injected, so a test can prove it is applied
/// without waiting for the real one.
async fn send_test_with(
    hook: &db::Webhook,
    body: &str,
    timeout: Duration,
) -> WebhookTestResult {
    let error = match attempt_once_within(hook, body, Some(timeout)).await {
        Ok(response) => {
            return WebhookTestResult {
                success: true,
                status_code: Some(
                    response
                        .status()
                        .as_u16(),
                ),
                error: None,
            };
        }
        Err(e) => e,
    };

    // Server-side only, and the same redaction guarantee as `deliver_logged`:
    // no URL path or query, ever.
    warn!(
        webhook = %hook.name,
        webhook_id = %hook.id,
        endpoint = %redact_url(&hook.url),
        error = %error,
        "webhook test delivery failed"
    );

    match error {
        // Status only: `e.message` carries part of the remote body.
        SendError {
            status: Some(status),
            ..
        } => WebhookTestResult {
            success: false,
            status_code: Some(status.as_u16()),
            error: Some(format!("endpoint returned {status}")),
        },
        // Nothing reached the endpoint, so the message is ours to give.
        e => WebhookTestResult {
            success: false,
            status_code: None,
            error: Some(e.to_string()),
        },
    }
}

/// One POST under the client's own [`REQUEST_TIMEOUT`].
async fn attempt_once(
    hook: &db::Webhook,
    body: &str,
) -> Result<reqwest::Response, SendError> {
    attempt_once_within(hook, body, None).await
}

/// One POST, classified.
///
/// `reqwest` treats a 4xx/5xx as a perfectly good response, so the status is
/// checked here. Redirects are not followed (see [`WEBHOOK_CLIENT`]), so a 3xx
/// lands in the same non-2xx branch.
async fn attempt_once_within(
    hook: &db::Webhook,
    body: &str,
    timeout: Option<Duration>,
) -> Result<reqwest::Response, SendError> {
    let shaped = shape_request(hook, body);
    let mut request = WEBHOOK_CLIENT
        .post(&hook.url)
        .header(CONTENT_TYPE, shaped.content_type);
    for (name, value) in shaped.headers {
        request = request.header(name, value);
    }
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
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
    /// got that far. Reported by [`send_test`].
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
/// 429 on purpose: Discord attaches these headers to responses generally, so
/// honouring them on a 5xx would collapse the backoff into an immediate burst.
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
/// Only the delta-seconds form is understood — that is what Discord sends.
/// Anything else yields `None` and the normal backoff applies.
///
/// The cap is applied to the `f64` **before** the conversion:
/// `Duration::from_secs_f64` panics outside `Duration`'s range, and this value
/// comes straight off a remote response header.
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

// The backoff curve is `remux_utils::retry::backoff`, imported at the top: only
// the *decision* to retry is webhook-specific, the delay is not.

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
        // `Content-Type` is pulled out of the operator's headers: it describes
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
        // of its own.
        WebhookDestination::Discord { .. } => ShapedRequest {
            body: rendered.to_string(),
            content_type: HeaderValue::from_static(JSON_CONTENT_TYPE),
            headers: Vec::new(),
        },
    }
}

/// The content type to send when the operator has not named one. A deviation
/// from the plugin, which sends everything as `text/plain`.
///
/// Validated, not deserialized: only the parse's success matters, so
/// `IgnoredAny` runs the same parser without building a `Value` tree.
pub(crate) fn detect_content_type(body: &str) -> &'static str {
    if serde_json::from_str::<serde::de::IgnoredAny>(body).is_ok() {
        JSON_CONTENT_TYPE
    } else {
        TEXT_CONTENT_TYPE
    }
}

// --- redaction --------------------------------------------------------------

/// Scheme and host only: a webhook URL's path is a credential (Discord's is
/// `.../webhooks/{id}/{token}`), so nothing past the host may reach a log.
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
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

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

    /// A local endpoint that answers `statuses` by call count, not by wall
    /// clock, repeating the last one once the list runs out. `httpmock` cannot
    /// do this, and swapping mocks mid-retry races the backoff.
    async fn sequenced_endpoint(
        statuses: &'static [u16],
    ) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move || {
                let counter = Arc::clone(&counter);
                async move {
                    let nth = counter.fetch_add(1, Ordering::SeqCst);
                    let status = statuses[nth.min(
                        statuses
                            .len()
                            .saturating_sub(1),
                    )];
                    axum::http::StatusCode::from_u16(status)
                        .expect("the test statuses must be valid")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free local port");
        let addr = listener
            .local_addr()
            .expect("the listener must have an address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}/hook"), calls)
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

    /// A Discord webhook token is the entire credential and must never reach a
    /// log line.
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
        assert_eq!(redact_url("not a url"), "<unparseable url>");
    }

    /// The transport error's own `Display` embeds the URL.
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
    /// Discord payload, so the sender must post it byte for byte.
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

    /// Matches on the received bytes, so deleting the header loop in
    /// `attempt_once` — which would silently drop an operator's auth token from
    /// every delivery — fails here.
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

    /// The value is remote input and the waiter holds a delivery slot.
    #[test]
    fn parse_retry_after_rejects_what_it_cannot_trust() {
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("NaN"), None);
        assert_eq!(parse_retry_after("999999"), Some(MAX_RETRY_AFTER));
    }

    /// These all parse as finite, positive floats, so they reach
    /// `Duration::from_secs_f64` — which panics outside `Duration`'s range.
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

    /// Discord attaches rate-limit headers to responses generally.
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

    // --- send_test: what reaches the admin API -----------------------------

    /// The hook's URL, headers and body are all admin-controlled and no host
    /// policy restricts them, so returning the endpoint's *response body* would
    /// make the test button a read primitive. Only the status may come back.
    #[tokio::test]
    async fn send_test_reports_the_status_without_the_remote_response_body() {
        let server = MockServer::start_async().await;
        let secret = "consul-token=s3cret internal detail";
        let mock = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(500)
                    .body(secret);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let result = send_test(&hook, "ping").await;

        assert!(!result.success);
        assert_eq!(result.status_code, Some(500));
        let error = result
            .error
            .expect("a failed test must carry an error");
        assert_eq!(error, "endpoint returned 500 Internal Server Error");
        assert!(
            !error.contains("consul-token"),
            "the remote body must not reach the caller: {error}"
        );
        assert!(!error.contains("internal detail"), "{error}");
        mock.assert_hits_async(1)
            .await;
    }

    /// A `tracing` subscriber that keeps what was written, scoped to the
    /// current thread so parallel tests do not see each other's lines.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8_lossy(
                &self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
            .into_owned()
        }
    }

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The caller only ever gets the status, so the endpoint's reason has to
    /// reach the server log instead — and the credential still must not.
    #[tokio::test]
    async fn a_failed_test_is_logged_server_side_without_the_url_path() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/api/webhooks/1/s3cret-token");
                then.status(400)
                    .body("Invalid Form Body: embeds.0.thumbnail.url: Not a well formed URL.");
            })
            .await;

        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();

        let hook = generic(&server.url("/api/webhooks/1/s3cret-token"), &[]);
        let result = {
            let _guard = tracing::subscriber::set_default(subscriber);
            send_test(&hook, "ping").await
        };

        assert!(!result.success);
        let logged = logs.text();
        assert!(
            logged.contains("webhook test delivery failed"),
            "the failed test must reach the server log: {logged:?}"
        );
        assert!(
            logged.contains("Not a well formed URL"),
            "the operator needs the endpoint's reason, in the log: {logged:?}"
        );
        assert!(
            !logged.contains("s3cret-token"),
            "the URL path is a credential and must not be logged: {logged:?}"
        );
        assert!(
            !result
                .error
                .unwrap_or_default()
                .contains("Not a well formed URL"),
            "…and the remote body still must not travel back to the caller"
        );
    }

    #[tokio::test]
    async fn send_test_reports_a_2xx_as_a_success() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(202);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let result = send_test(&hook, "ping").await;
        assert!(result.success);
        assert_eq!(result.status_code, Some(202));
        assert_eq!(result.error, None);
    }

    /// A blackholing endpoint must not hold an admin request handler for the
    /// full [`REQUEST_TIMEOUT`].
    #[tokio::test]
    async fn send_test_applies_its_own_timeout_to_the_request() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(200)
                    .delay(Duration::from_secs(5));
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let started = Instant::now();
        let result = send_test_with(&hook, "ping", Duration::from_millis(100)).await;
        let elapsed = started.elapsed();

        assert!(!result.success, "a timed-out request is not a success");
        assert_eq!(result.status_code, None, "nothing answered");
        assert!(
            elapsed < Duration::from_secs(2),
            "the per-request timeout must fire long before the response: {elapsed:?}"
        );
    }

    // --- redirects ----------------------------------------------------------

    /// `reqwest` follows up to ten redirects by default, which would let the
    /// remote server pick a host reached from inside this server's network.
    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let server = MockServer::start_async().await;
        let redirect = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(302)
                    .header("location", "/internal");
            })
            .await;
        let target = server
            .mock_async(|when, then| {
                when.path("/internal");
                then.status(200)
                    .body("secrets");
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        let result = send_test(&hook, "ping").await;

        redirect
            .assert_hits_async(1)
            .await;
        target
            .assert_hits_async(0)
            .await;
        assert!(!result.success, "a 302 is not a delivered webhook");
        assert_eq!(result.status_code, Some(302));
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_or_retried_during_delivery() {
        let server = MockServer::start_async().await;
        let redirect = server
            .mock_async(|when, then| {
                when.path("/hook");
                then.status(302)
                    .header("location", "/internal");
            })
            .await;
        let target = server
            .mock_async(|when, then| {
                when.path("/internal");
                then.status(200);
            })
            .await;

        let hook = generic(&server.url("/hook"), &[]);
        deliver_with(&hook, "ping", &FAST)
            .await
            .expect_err("a redirect is not a delivery");

        redirect
            .assert_hits_async(1)
            .await;
        target
            .assert_hits_async(0)
            .await;
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

    /// Repeating a 4xx verbatim cannot help, and on Discord it burns rate limit.
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

    /// The exponential schedule has to survive an endpoint that advertises a
    /// zero rate-limit reset while failing for an unrelated reason.
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
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "the backoff was skipped, took only {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn delivery_stops_at_the_first_success() {
        let (url, calls) = sequenced_endpoint(&[500, 500, 200]).await;

        let hook = generic(&url, &[]);
        deliver_with(&hook, "ping", &FAST)
            .await
            .expect("the third attempt succeeded, so the delivery must succeed");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "the retry must stop at the first success, not spend the budget"
        );
    }

    #[tokio::test]
    async fn a_success_ends_the_retry_loop_with_budget_to_spare() {
        let (url, calls) = sequenced_endpoint(&[500, 200, 500]).await;

        let hook = generic(&url, &[]);
        deliver_with(
            &hook,
            "ping",
            &DeliveryPolicy {
                attempts: 3,
                retry_delay_ms: 20,
            },
        )
        .await
        .expect("the second attempt succeeded");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the third attempt must never have been made"
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

    /// A saturated endpoint must not consume the slots of a healthy one.
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

    #[test]
    fn retain_forgets_hooks_that_are_gone() {
        let slots = DeliverySlots::new(2);
        let live = Uuid::from_u128(1);
        let removed = Uuid::from_u128(2);
        drop(
            slots
                .try_acquire(live)
                .expect("a fresh hook has slots"),
        );
        drop(
            slots
                .try_acquire(removed)
                .expect("a fresh hook has slots"),
        );

        slots.retain(&HashSet::from([live]));

        let per_hook = slots
            .per_hook
            .lock()
            .expect("uncontended");
        assert!(per_hook.contains_key(&live));
        assert!(
            !per_hook.contains_key(&removed),
            "a hook no longer in the live set must not keep its entry"
        );
    }

    /// Evicting an entry whose permit is still out would let the next delivery
    /// build a second semaphore and exceed the per-hook limit.
    #[test]
    fn retain_keeps_a_hook_with_a_delivery_still_in_flight() {
        let slots = DeliverySlots::new(1);
        let removed = Uuid::from_u128(2);
        let permit = slots
            .try_acquire(removed)
            .expect("a fresh hook has slots");

        slots.retain(&HashSet::new());
        assert!(
            slots
                .try_acquire(removed)
                .is_none(),
            "the in-flight permit must still be counted after a prune"
        );

        drop(permit);
        slots.retain(&HashSet::new());
        assert!(
            !slots
                .per_hook
                .lock()
                .expect("uncontended")
                .contains_key(&removed),
            "a later prune collects it once the permit is back"
        );
    }

    /// The drop branch drops rather than queueing: a saturated hook must
    /// produce no request at all.
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

    /// [`LogThrottle`] is unit-tested in its own module; this proves it is
    /// wired into the drop branch, which is reachable from an *unauthenticated*
    /// caller (`AuthenticationFailure` on a failed login).
    #[tokio::test]
    async fn the_saturation_warning_is_logged_once_not_once_per_dropped_event() {
        let slots = DeliverySlots::new(1);
        // A hook id of its own: the throttle is process-wide, so sharing one
        // with another test would make this depend on execution order.
        let hook = db::Webhook {
            id: Uuid::from_u128(0x5a7a_1a7e),
            ..generic("http://127.0.0.1:1/hook", &[])
        };
        let _held = slots
            .try_acquire(hook.id)
            .expect("a fresh hook has a slot");

        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        {
            let _guard = tracing::subscriber::set_default(subscriber);
            for _ in 0..50 {
                assert!(
                    !spawn_delivery_with(&slots, hook.clone(), "ping".into(), FAST),
                    "with no slot every one of these must be dropped"
                );
            }
        }

        let logged = logs.text();
        assert_eq!(
            logged
                .matches("dropping event")
                .count(),
            1,
            "fifty drops must produce one line, not fifty: {logged:?}"
        );
        assert!(
            logged.contains("dropped_since=0"),
            "the line must carry the count it stands for: {logged:?}"
        );
    }

    /// The permit is taken *before* the spawn: acquiring it inside the task
    /// would bound sockets but let tasks pile up parked on the semaphore.
    /// Nothing is awaited before the assertion, so the spawned task cannot have
    /// run — this observes the synchronous acquisition only.
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

        eventually("the slot to come back", async || {
            slots
                .try_acquire(hook.id)
                .is_some()
        })
        .await;
    }
}
