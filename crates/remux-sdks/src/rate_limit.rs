//! Honour `Retry-After` when an upstream returns HTTP 429.
//!
//! `reqwest-retry` classifies 429 as transient and retries it, but its
//! `RetryPolicy::should_retry` only receives the start time and attempt count —
//! the response never reaches it, so the one header whose entire purpose is to
//! say *how long* to wait is ignored and the request is reissued on a generic
//! exponential curve (~1s, 2s, 4s). Against an upstream that just asked for 60
//! seconds that is worse than not retrying at all.
//!
//! `RetryAfterMiddleware` sits inside the retry middleware and simply sleeps
//! for the requested delay before handing the 429 back. Any retry attempt above
//! it therefore pays the delay first, which spaces attempts the way the server
//! asked without a second retry loop. Because the caller's task is parked for
//! the duration, the existing concurrency limit becomes the effective rate
//! limit — no shared per-origin state, so clients using different credentials
//! against the same host never block one another.
//!
//! Retry-After parsing (delay-seconds *and* HTTP-date) is from @DaanVT86's
//! work in #399.

use http::{HeaderMap, header};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// RFC 6585 §4 makes `Retry-After` optional on 429, so we need a stand-in.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Upper bound on how long one response may park a request. `Retry-After` is
/// attacker- or bug-controlled: uncapped, a single `Retry-After: 999999999`
/// (or a far-future date) would wedge the caller for ~31 years with no way to
/// clear it short of a restart. Backing off for minutes is the useful part;
/// anything beyond that is better surfaced as an error the caller can act on.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

/// Delay requested by a 429 response, clamped to [`MAX_RETRY_AFTER`], falling
/// back to [`DEFAULT_RETRY_AFTER`] when the header is absent or unparseable.
pub(crate) fn retry_after(headers: &HeaderMap, now: SystemTime) -> Duration {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| {
            value
                .to_str()
                .ok()
        })
        .and_then(|value| parse_retry_after(value, now))
        .unwrap_or(DEFAULT_RETRY_AFTER)
        .min(MAX_RETRY_AFTER)
}

/// `Retry-After` is either delay-seconds or an HTTP-date (RFC 9110 §10.2.3).
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = chrono::DateTime::parse_from_rfc2822(value)
        .map(|date| date.timestamp())
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(value, "%a, %d %b %Y %H:%M:%S GMT")
                .map(|date| {
                    date.and_utc()
                        .timestamp()
                })
        })
        .ok()?;
    let retry_at = u64::try_from(retry_at).unwrap_or_default();
    let now = now
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    // A date already in the past means "retry now", not "wait the default".
    Some(Duration::from_secs(retry_at.saturating_sub(now)))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RetryAfterMiddleware;

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl reqwest_middleware::Middleware for RetryAfterMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let response = next
            .run(req, extensions)
            .await?;
        if response.status() != http::StatusCode::TOO_MANY_REQUESTS {
            return Ok(response);
        }

        let delay = retry_after(response.headers(), SystemTime::now());
        if delay.is_zero() {
            return Ok(response);
        }
        tracing::warn!(
            url = %response.url(),
            retry_after_secs = delay.as_secs(),
            "upstream returned 429; backing off before this request returns"
        );
        tokio::time::sleep(delay).await;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_delay_seconds() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        assert_eq!(parse_retry_after("42", now), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parses_http_date() {
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_450);
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn a_date_in_the_past_means_no_wait() {
        let now = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn defaults_to_sixty_seconds_when_absent_or_unparseable() {
        assert_eq!(
            retry_after(&HeaderMap::new(), UNIX_EPOCH),
            DEFAULT_RETRY_AFTER
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "not-a-delay"
                .parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, UNIX_EPOCH), DEFAULT_RETRY_AFTER);
    }

    #[test]
    fn clamps_an_absurd_retry_after() {
        // Uncapped this would park the caller for ~31 years.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "999999999"
                .parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, UNIX_EPOCH), MAX_RETRY_AFTER);
    }

    #[test]
    fn clamps_an_absurd_future_date() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "Fri, 01 Jan 2100 00:00:00 GMT"
                .parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, UNIX_EPOCH), MAX_RETRY_AFTER);
    }

    #[test]
    fn honours_a_delay_under_the_cap() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "5".parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, UNIX_EPOCH), Duration::from_secs(5));
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod middleware {
        use crate::{ClientError, Endpoint, RestClient};
        use std::time::{Duration, Instant};

        #[derive(Clone)]
        struct Probe(&'static str);

        impl Endpoint for Probe {
            type Output = Vec<String>;

            fn path(&self) -> String {
                self.0
                    .to_string()
            }
        }

        #[tokio::test(flavor = "current_thread")]
        async fn a_429_parks_the_caller_for_the_requested_delay() {
            let server = httpmock::MockServer::start();
            let limited = server.mock(|when, then| {
                when.path("/limited");
                then.status(429)
                    .header("Retry-After", "1");
            });
            let client = RestClient::new(&server.base_url()).unwrap();

            let started = Instant::now();
            let error = client
                .execute(Probe("/limited"))
                .await
                .unwrap_err();
            let elapsed = started.elapsed();

            match error {
                ClientError::RateLimited { retry_after_secs } => {
                    assert_eq!(retry_after_secs, 1)
                }
                other => panic!("expected rate-limit error, got {other}"),
            }
            assert!(
                elapsed >= Duration::from_millis(750),
                "caller should have been parked for the requested delay, took {elapsed:?}"
            );
            // The point of parking is that the upstream sees one request, not a
            // burst of immediate retries.
            assert_eq!(limited.hits(), 1);
        }

        #[tokio::test(flavor = "current_thread")]
        async fn a_successful_response_is_never_delayed() {
            let server = httpmock::MockServer::start();
            let ok = server.mock(|when, then| {
                when.path("/ok");
                then.status(200)
                    .json_body(serde_json::json!([]));
            });
            let client = RestClient::new(&server.base_url()).unwrap();

            let started = Instant::now();
            client
                .execute(Probe("/ok"))
                .await
                .unwrap();

            assert!(started.elapsed() < Duration::from_millis(500));
            assert_eq!(ok.hits(), 1);
        }

        #[tokio::test(flavor = "current_thread")]
        async fn clients_for_different_origins_do_not_block_each_other() {
            // No shared per-origin state: a 429 from one host must not stall a
            // client talking to another (or to the same host under different
            // credentials).
            let limited_server = httpmock::MockServer::start();
            limited_server.mock(|when, then| {
                when.path("/limited");
                then.status(429)
                    .header("Retry-After", "30");
            });
            let other_server = httpmock::MockServer::start();
            let other = other_server.mock(|when, then| {
                when.path("/ok");
                then.status(200)
                    .json_body(serde_json::json!([]));
            });

            let limited_client = RestClient::new(&limited_server.base_url()).unwrap();
            let other_client = RestClient::new(&other_server.base_url()).unwrap();

            let limited = tokio::spawn(async move {
                let _ = limited_client
                    .execute(Probe("/limited"))
                    .await;
            });

            let started = Instant::now();
            other_client
                .execute(Probe("/ok"))
                .await
                .unwrap();
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "unrelated origin was blocked by another host's 429"
            );
            assert_eq!(other.hits(), 1);
            limited.abort();
        }
    }
}
