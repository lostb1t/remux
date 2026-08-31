//! Shared HTTP 429 backpressure for all outbound SDK clients.

use http::{HeaderMap, header};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);

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
use {
    async_trait::async_trait,
    reqwest_middleware::{Middleware, Next},
    std::{
        collections::HashMap,
        sync::{LazyLock, Mutex},
        time::Instant,
    },
};

#[cfg(not(target_arch = "wasm32"))]
static RATE_LIMIT_COOLDOWNS: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(not(target_arch = "wasm32"))]
fn origin(url: &url::Url) -> String {
    url.origin()
        .ascii_serialization()
}

#[cfg(not(target_arch = "wasm32"))]
async fn wait_for_origin(origin: &str) {
    loop {
        let delay = {
            let now = Instant::now();
            let mut cooldowns = RATE_LIMIT_COOLDOWNS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match cooldowns
                .get(origin)
                .copied()
            {
                Some(deadline) if deadline > now => Some(deadline.duration_since(now)),
                Some(_) => {
                    cooldowns.remove(origin);
                    None
                }
                None => None,
            }
        };

        match delay {
            Some(delay) => tokio::time::sleep(delay).await,
            None => return,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pause_origin(origin: &str, delay: Duration) -> bool {
    if delay.is_zero() {
        return false;
    }

    let now = Instant::now();
    let Some(deadline) = now.checked_add(delay) else {
        return false;
    };
    let mut cooldowns = RATE_LIMIT_COOLDOWNS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cooldowns.retain(|_, current| *current > now);

    let extends_cooldown = match cooldowns.get(origin) {
        Some(current) => deadline > *current,
        None => true,
    };
    if extends_cooldown {
        cooldowns.insert(origin.to_string(), deadline);
    }
    extends_cooldown
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RateLimitMiddleware;

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Middleware for RateLimitMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let request_origin = origin(req.url());
        wait_for_origin(&request_origin).await;

        let response = next
            .run(req, extensions)
            .await?;
        if response.status() == http::StatusCode::TOO_MANY_REQUESTS {
            let delay = retry_after(response.headers(), SystemTime::now());
            let response_origin = origin(response.url());
            let request_extended = pause_origin(&request_origin, delay);
            let response_extended = response_origin != request_origin
                && pause_origin(&response_origin, delay);

            if request_extended || response_extended {
                tracing::warn!(
                    origin = %response_origin,
                    retry_after_secs = delay.as_secs(),
                    "upstream returned 429; pausing requests for origin"
                );
            }
        }

        Ok(response)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{ClientError, Endpoint, RestClient};

    #[derive(Clone)]
    struct Probe(&'static str);

    impl Endpoint for Probe {
        type Output = Vec<String>;

        fn path(&self) -> String {
            self.0
                .to_string()
        }
    }

    #[test]
    fn parses_retry_after_delay_seconds() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        assert_eq!(parse_retry_after("42", now), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parses_retry_after_http_date() {
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_450);
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn defaults_to_sixty_seconds_for_missing_or_invalid_header() {
        let headers = HeaderMap::new();
        assert_eq!(retry_after(&headers, UNIX_EPOCH), DEFAULT_RETRY_AFTER);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::RETRY_AFTER,
            "not-a-delay"
                .parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, UNIX_EPOCH), DEFAULT_RETRY_AFTER);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cooldown_is_shared_between_clients_for_the_same_origin() {
        let server = httpmock::MockServer::start();
        let limited = server.mock(|when, then| {
            when.path("/limited");
            then.status(429)
                .header("Retry-After", "1");
        });
        let ok = server.mock(|when, then| {
            when.path("/ok");
            then.status(200)
                .json_body(serde_json::json!([]));
        });
        let first = RestClient::new(&server.base_url()).unwrap();
        let second = RestClient::new(&server.base_url()).unwrap();

        let error = first
            .execute(Probe("/limited"))
            .await
            .unwrap_err();
        match error {
            ClientError::RateLimited { retry_after_secs } => {
                assert_eq!(retry_after_secs, 1);
            }
            other => panic!("expected rate-limit error, got {other}"),
        }

        let started = std::time::Instant::now();
        second
            .execute(Probe("/ok"))
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_millis(750),
            "request was sent before the shared cooldown elapsed: {elapsed:?}"
        );
        assert_eq!(limited.hits(), 1);
        assert_eq!(ok.hits(), 1);
    }
}
