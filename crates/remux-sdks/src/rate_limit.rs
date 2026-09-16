use http::{HeaderMap, header};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use tokio::{sync::Mutex, time::Instant};

pub(crate) const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);

const MAX_RETRY_AFTER: Duration = Duration::from_secs(48 * 60 * 60);

/// Shared 429 cooldown for one provider.
///
/// Give clones of one value to every [`RestClient`](crate::RestClient) that
/// talks to the same upstream. After a 429, it parks all later requests until
/// the shared cooldown has elapsed. It intentionally does not limit request
/// concurrency; callers control that themselves.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct SharedRateLimit {
    blocked_until: Arc<Mutex<Instant>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SharedRateLimit {
    pub fn new() -> Self {
        Self {
            blocked_until: Arc::new(Mutex::new(Instant::now())),
        }
    }

    async fn block_for(&self, delay: Duration) {
        let candidate = Instant::now() + delay;
        let mut blocked_until = self
            .blocked_until
            .lock()
            .await;
        *blocked_until = (*blocked_until).max(candidate);
    }

    async fn wait_for_cooldown(&self) {
        loop {
            let blocked_until = *self
                .blocked_until
                .lock()
                .await;
            let now = Instant::now();
            if blocked_until <= now {
                return;
            }
            tokio::time::sleep_until(blocked_until).await;
        }
    }
}

pub(crate) fn retry_after(
    headers: &HeaderMap,
    now: SystemTime,
    default: Duration,
) -> Duration {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| {
            value
                .to_str()
                .ok()
        })
        .and_then(|value| parse_retry_after(value, now))
        .unwrap_or(default)
        .min(MAX_RETRY_AFTER)
}

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
    Some(Duration::from_secs(retry_at.saturating_sub(now)))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RetryAfterMiddleware {
    pub(crate) default_retry_after: Duration,
    pub(crate) shared_rate_limit: Option<SharedRateLimit>,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl reqwest_middleware::Middleware for RetryAfterMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        if let Some(limit) = &self.shared_rate_limit {
            limit
                .wait_for_cooldown()
                .await;
        }
        let response = next
            .run(req, extensions)
            .await?;
        if response.status() != http::StatusCode::TOO_MANY_REQUESTS {
            return Ok(response);
        }

        let delay = retry_after(
            response.headers(),
            SystemTime::now(),
            self.default_retry_after,
        );
        if delay.is_zero() {
            return Ok(response);
        }
        tracing::warn!(
            url = %response.url(),
            retry_after_secs = delay.as_secs(),
            shared = self.shared_rate_limit.is_some(),
            "upstream returned 429; backing off before this request returns"
        );
        if let Some(limit) = &self.shared_rate_limit {
            limit
                .block_for(delay)
                .await;
            limit
                .wait_for_cooldown()
                .await;
        } else {
            tokio::time::sleep(delay).await;
        }
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
    fn defaults_to_the_configured_default_when_absent_or_unparseable() {
        assert_eq!(
            retry_after(&HeaderMap::new(), UNIX_EPOCH, DEFAULT_RETRY_AFTER),
            DEFAULT_RETRY_AFTER
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "not-a-delay"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            retry_after(&headers, UNIX_EPOCH, Duration::from_secs(2)),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn clamps_an_absurd_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "999999999"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            retry_after(&headers, UNIX_EPOCH, DEFAULT_RETRY_AFTER),
            MAX_RETRY_AFTER
        );
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
        assert_eq!(
            retry_after(&headers, UNIX_EPOCH, DEFAULT_RETRY_AFTER),
            MAX_RETRY_AFTER
        );
    }

    #[test]
    fn honours_a_multi_hour_backoff() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "21600"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            retry_after(&headers, UNIX_EPOCH, DEFAULT_RETRY_AFTER),
            Duration::from_secs(21600)
        );
    }

    #[test]
    fn honours_a_delay_under_the_cap() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "5".parse()
                .unwrap(),
        );
        assert_eq!(
            retry_after(&headers, UNIX_EPOCH, DEFAULT_RETRY_AFTER),
            Duration::from_secs(5)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod shared_rate_limit {
        use super::super::SharedRateLimit;
        use std::time::Duration;
        use tokio::time::Instant;

        #[tokio::test(flavor = "current_thread", start_paused = true)]
        async fn a_fresh_limit_never_blocks() {
            let limit = SharedRateLimit::new();
            let started = Instant::now();
            limit
                .wait_for_cooldown()
                .await;
            assert_eq!(
                started.elapsed(),
                Duration::ZERO,
                "a limit that was never tripped must not delay callers"
            );
        }

        #[tokio::test(flavor = "current_thread", start_paused = true)]
        async fn tripping_the_limit_parks_a_later_waiter_for_the_delay() {
            let limit = SharedRateLimit::new();
            limit
                .block_for(Duration::from_secs(5))
                .await;

            let started = Instant::now();
            limit
                .wait_for_cooldown()
                .await;
            assert_eq!(started.elapsed(), Duration::from_secs(5));
        }

        #[tokio::test(flavor = "current_thread", start_paused = true)]
        async fn a_shorter_block_never_shrinks_an_existing_longer_cooldown() {
            let limit = SharedRateLimit::new();
            limit
                .block_for(Duration::from_secs(10))
                .await;
            // A second, shorter delay (e.g. from a request that raced the first
            // 429 and got a smaller Retry-After) must not pull the deadline in —
            // whoever asked for the longest cooldown wins.
            limit
                .block_for(Duration::from_secs(1))
                .await;

            let started = Instant::now();
            limit
                .wait_for_cooldown()
                .await;
            assert_eq!(
                started.elapsed(),
                Duration::from_secs(10),
                "a shorter subsequent block must not shrink the cooldown"
            );
        }

        #[tokio::test(flavor = "current_thread", start_paused = true)]
        async fn a_longer_block_extends_an_existing_shorter_cooldown() {
            let limit = SharedRateLimit::new();
            limit
                .block_for(Duration::from_secs(1))
                .await;
            limit
                .block_for(Duration::from_secs(10))
                .await;

            let started = Instant::now();
            limit
                .wait_for_cooldown()
                .await;
            assert_eq!(started.elapsed(), Duration::from_secs(10));
        }

        #[tokio::test(flavor = "current_thread", start_paused = true)]
        async fn concurrent_waiters_all_release_at_the_same_deadline() {
            let limit = SharedRateLimit::new();
            limit
                .block_for(Duration::from_secs(3))
                .await;

            let started = Instant::now();
            let (a, b) =
                tokio::join!(limit.wait_for_cooldown(), limit.wait_for_cooldown());
            let _: ((), ()) = (a, b);
            assert_eq!(started.elapsed(), Duration::from_secs(3));
        }
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

        #[tokio::test(flavor = "current_thread")]
        async fn clients_sharing_a_rate_limit_block_each_other_after_a_429() {
            use crate::rate_limit::SharedRateLimit;

            let limited_server = httpmock::MockServer::start();
            limited_server.mock(|when, then| {
                when.path("/limited");
                then.status(429)
                    .header("Retry-After", "1");
            });
            let other_server = httpmock::MockServer::start();
            let other = other_server.mock(|when, then| {
                when.path("/ok");
                then.status(200)
                    .json_body(serde_json::json!([]));
            });

            let shared = SharedRateLimit::new();
            let limited_client = RestClient::new(&limited_server.base_url())
                .unwrap()
                .with_shared_rate_limit(shared.clone());
            // Same shared limit, entirely different underlying host — mirrors
            // two independently-constructed clients for one logical provider
            // (see `tmdb_rate_limit`), not two requests to the same server.
            let other_client = RestClient::new(&other_server.base_url())
                .unwrap()
                .with_shared_rate_limit(shared);

            // Spawned so it races the second request instead of fully waiting
            // out its own cooldown before this task even starts the other
            // call — otherwise the shared deadline would already be in the
            // past by the time `other_client` checked it.
            let limited = tokio::spawn(async move {
                let _ = limited_client
                    .execute(Probe("/limited"))
                    .await;
            });
            // Give the spawned request enough of a head start to trip the
            // limit before this one checks it.
            tokio::time::sleep(Duration::from_millis(100)).await;

            let started = Instant::now();
            other_client
                .execute(Probe("/ok"))
                .await
                .unwrap();
            assert!(
                started.elapsed() >= Duration::from_millis(700),
                "a client sharing the tripped limit should have been parked too, took {:?}",
                started.elapsed()
            );
            assert_eq!(other.hits(), 1);
            limited
                .await
                .unwrap();
        }
    }
}
