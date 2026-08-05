//! HTTP delivery of a rendered webhook body.
//!
//! Everything that decides *what* goes on the wire lives in pure functions
//! ([`shape_request`], [`build_discord_body`], [`parse_embed_color`],
//! [`detect_content_type`]); [`send_once`] only performs the POST. A new
//! destination is a new `WebhookDestination` variant plus an arm in
//! [`shape_request`].
//!
//! Delivery is fire-and-forget: [`spawn_delivery`] never blocks its caller and
//! every error is logged and swallowed, so a broken endpoint can neither stall
//! the dispatcher nor surface anywhere in the server.

use crate::db;
use remux_sdks::remux::{DiscordMentionType, WebhookDestination};
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

/// Per-request timeout. Without one an endpoint that accepts the connection and
/// then blackholes it would hold its task — and its concurrency slot — forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on deliveries in flight at once. A dead endpoint plus a sustained
/// `PlaybackProgress` stream would otherwise spawn tasks without bound; past
/// this many, events are dropped with a warning rather than queued.
const MAX_CONCURRENT_DELIVERIES: usize = 16;

/// Discord's own default embed colour, as used by the Jellyfin webhook plugin's
/// stock Discord templates (`0x3399FF`).
const DEFAULT_EMBED_COLOR: u32 = 3_381_759;

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

static DELIVERY_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DELIVERIES)));

/// How hard a single delivery tries. Extracted so tests can shrink the backoff.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeliveryPolicy {
    pub attempts: u32,
    /// Base delay in milliseconds; `retry!` grows it exponentially with jitter.
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

/// Hand a rendered body to the delivery pool.
///
/// Returns immediately. When every slot is busy the delivery is dropped rather
/// than queued: an unbounded backlog behind a dead endpoint is worse than a
/// missed notification, and the dispatcher must never wait here.
pub(crate) fn spawn_delivery(hook: db::Webhook, body: String) {
    let Ok(permit) = DELIVERY_SLOTS
        .clone()
        .try_acquire_owned()
    else {
        warn!(
            webhook = %hook.name,
            limit = MAX_CONCURRENT_DELIVERIES,
            "webhook delivery slots exhausted, dropping event"
        );
        return;
    };
    tokio::spawn(async move {
        // Held for the whole delivery, retries included.
        let _permit = permit;
        deliver(hook, body).await;
    });
}

/// Deliver `body` to `hook`, retrying transient failures. Never fails: a broken
/// webhook is a log line, nothing more.
pub(crate) async fn deliver(hook: db::Webhook, body: String) {
    if let Err(e) = deliver_with(&hook, &body, &DeliveryPolicy::default()).await {
        warn!(webhook = %hook.name, url = %hook.url, error = %e, "webhook delivery failed, giving up");
    }
}

/// The retried delivery, with its outcome still visible. `deliver` is this plus
/// the logging.
pub(crate) async fn deliver_with(
    hook: &db::Webhook,
    body: &str,
    policy: &DeliveryPolicy,
) -> anyhow::Result<()> {
    let response = remux_utils::retry! {
        attempts: policy.attempts,
        delay: policy.retry_delay_ms,
        { send_once(hook, body).await }
    }?;
    debug!(
        webhook = %hook.name,
        status = %response.status().as_u16(),
        "webhook delivered"
    );
    Ok(())
}

/// A single POST.
///
/// `reqwest` treats a 4xx/5xx as a perfectly good response, so the status is
/// checked here: without this every failed delivery would be reported as a
/// success and the retry would never fire.
pub(crate) async fn send_once(
    hook: &db::Webhook,
    body: &str,
) -> anyhow::Result<reqwest::Response> {
    let shaped = shape_request(hook, body);
    let mut request = WEBHOOK_CLIENT
        .post(&hook.url)
        .header(CONTENT_TYPE, shaped.content_type);
    for (name, value) in shaped.headers {
        request = request.header(name, value);
    }
    // An unparseable URL surfaces here as an error, not a panic.
    let response = request
        .body(shaped.body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let detail = response
            .text()
            .await
            .unwrap_or_default();
        anyhow::bail!(
            "webhook endpoint returned {status}: {}",
            truncate(detail.trim(), MAX_LOGGED_RESPONSE)
        );
    }
    Ok(response)
}

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
        // The rendered body is the embed description; everything around it is
        // built from the destination's own settings.
        WebhookDestination::Discord {
            avatar_url,
            bot_username,
            embed_color,
            mention_type,
        } => ShapedRequest {
            body: build_discord_body(
                avatar_url.as_deref(),
                bot_username.as_deref(),
                embed_color.as_deref(),
                *mention_type,
                rendered,
            )
            .to_string(),
            content_type: HeaderValue::from_static(JSON_CONTENT_TYPE),
            headers: Vec::new(),
        },
    }
}

/// The Discord execute-webhook payload wrapping `rendered`.
///
/// `content` carries the mention (empty for `None`); the rendered text becomes
/// the description of a single embed. `username` and `avatar_url` are omitted
/// when unset rather than sent empty.
pub(crate) fn build_discord_body(
    avatar_url: Option<&str>,
    bot_username: Option<&str>,
    embed_color: Option<&str>,
    mention_type: DiscordMentionType,
    rendered: &str,
) -> Value {
    let mut embed = Map::new();
    embed.insert("description".into(), Value::String(rendered.to_string()));
    embed.insert(
        "color".into(),
        Value::Number(parse_embed_color(embed_color).into()),
    );

    let mut body = Map::new();
    body.insert(
        "content".into(),
        Value::String(mention_content(mention_type).to_string()),
    );
    if let Some(username) = non_empty(bot_username) {
        body.insert("username".into(), Value::String(username.to_string()));
    }
    if let Some(avatar) = non_empty(avatar_url) {
        body.insert("avatar_url".into(), Value::String(avatar.to_string()));
    }
    body.insert("embeds".into(), Value::Array(vec![Value::Object(embed)]));
    Value::Object(body)
}

/// What a mention type puts in Discord's `content` field.
fn mention_content(mention_type: DiscordMentionType) -> &'static str {
    match mention_type {
        DiscordMentionType::None => "",
        DiscordMentionType::Here => "@here",
        DiscordMentionType::Everyone => "@everyone",
    }
}

/// `#RRGGBB` (or bare `RRGGBB`) as the integer Discord wants. Anything else —
/// including a missing colour — falls back to [`DEFAULT_EMBED_COLOR`]: the
/// value is operator input and must never fail a delivery.
pub(crate) fn parse_embed_color(hex: Option<&str>) -> u32 {
    hex.map(|hex| {
        hex.trim()
            .trim_start_matches('#')
    })
    .filter(|hex| {
        hex.len() == 6
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
    })
    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
    .unwrap_or(DEFAULT_EMBED_COLOR)
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

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
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
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};
    use uuid::Uuid;

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

    // --- parse_embed_color ------------------------------------------------

    #[test]
    fn parse_embed_color_reads_a_six_digit_hex() {
        assert_eq!(parse_embed_color(Some("#AA5CC3")), 11_164_867);
        // Lower case and a missing '#' are both accepted.
        assert_eq!(parse_embed_color(Some("aa5cc3")), 11_164_867);
        assert_eq!(parse_embed_color(Some("#000000")), 0);
        assert_eq!(parse_embed_color(Some("#FFFFFF")), 16_777_215);
    }

    #[test]
    fn parse_embed_color_falls_back_to_the_default() {
        for input in [
            None,
            Some(""),
            Some("#"),
            Some("#AA5CC"),   // too short
            Some("#AA5CC3F"), // too long
            Some("#GGGGGG"),  // not hex
            Some("rebeccapurple"),
        ] {
            assert_eq!(
                parse_embed_color(input),
                DEFAULT_EMBED_COLOR,
                "{input:?} must fall back to the default colour"
            );
        }
    }

    // --- build_discord_body -----------------------------------------------

    #[test]
    fn discord_body_content_carries_the_mention_type() {
        for (mention_type, expected) in [
            (DiscordMentionType::None, ""),
            (DiscordMentionType::Here, "@here"),
            (DiscordMentionType::Everyone, "@everyone"),
        ] {
            let body = build_discord_body(None, None, None, mention_type, "hi");
            assert_eq!(
                body["content"],
                json!(expected),
                "{mention_type:?} must produce {expected:?}"
            );
        }
    }

    #[test]
    fn discord_body_wraps_the_rendered_text_in_a_single_embed() {
        let body = build_discord_body(
            None,
            None,
            Some("#AA5CC3"),
            DiscordMentionType::None,
            "a line",
        );
        let embeds = body["embeds"]
            .as_array()
            .expect("embeds must be an array");
        assert_eq!(embeds.len(), 1);
        assert_eq!(embeds[0]["description"], json!("a line"));
        assert_eq!(embeds[0]["color"], json!(11_164_867));
    }

    #[test]
    fn discord_body_carries_the_bot_identity_only_when_set() {
        let with = build_discord_body(
            Some("https://example.test/a.png"),
            Some("remux"),
            None,
            DiscordMentionType::None,
            "hi",
        );
        assert_eq!(with["avatar_url"], json!("https://example.test/a.png"));
        assert_eq!(with["username"], json!("remux"));

        let without =
            build_discord_body(None, None, None, DiscordMentionType::None, "hi");
        assert!(
            !without
                .as_object()
                .unwrap()
                .contains_key("avatar_url"),
            "an unset avatar must not be sent as an empty string"
        );
        assert!(
            !without
                .as_object()
                .unwrap()
                .contains_key("username")
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

    #[test]
    fn discord_posts_the_envelope_as_json() {
        let hook =
            discord_hook("https://example.test/hook", DiscordMentionType::Everyone);
        let shaped = shape_request(&hook, "a line");
        assert!(
            content_type(&hook, "a line").starts_with("application/json"),
            "Discord always takes JSON"
        );
        let parsed: Value =
            serde_json::from_str(&shaped.body).expect("the body must be valid JSON");
        assert_eq!(parsed["content"], json!("@everyone"));
        assert_eq!(parsed["embeds"][0]["description"], json!("a line"));
        assert!(
            shaped
                .headers
                .is_empty()
        );
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while failing
            .hits_async()
            .await
            < 2
        {
            assert!(
                Instant::now() < deadline,
                "the retry never reached attempt 2"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
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
}
