/// `base_ms * 2^attempt` plus jitter in `[0, base_ms/2)`.
///
/// The one definition of the project's backoff curve, so [`retry!`] and the
/// hand-rolled loops that cannot use it (webhook delivery retries only *some*
/// failures) cannot drift apart.
///
/// `attempt` is 0-based; the exponent is capped and every step saturates.
pub fn backoff(base_ms: u64, attempt: u32) -> ::std::time::Duration {
    let exponential = base_ms.saturating_mul(1u64 << attempt.min(10));
    // `SystemTime` nanos as cheap entropy — this only needs to de-correlate
    // concurrent retriers, not resist prediction.
    let jitter = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos() as u64 % (base_ms / 2 + 1))
        .unwrap_or(0);
    ::std::time::Duration::from_millis(exponential.saturating_add(jitter))
}

/// Retry a fallible async expression with exponential backoff and jitter.
///
/// # Parameters
/// - `attempts` — max number of tries (default: 3)
/// - `delay` — base delay in milliseconds (default: 500)
/// - `{ block }` — expression that must evaluate to `Result<T, E>`
///
/// # Backoff
/// `sleep = base_delay * 2^attempt + jitter`
/// where jitter is a random value in `[0, base_delay / 2)` derived from
/// `SystemTime` nanoseconds.
///
/// # Example
/// ```rust,ignore
/// let resp = retry! {
///     attempts: 3,
///     delay: 500,
///     { client.get(url).send().await }
/// }?;
/// ```
#[macro_export]
macro_rules! retry {
    // Full form: all options provided
    (attempts: $attempts:expr, delay: $delay:expr, $body:block) => {{
        let mut __last_err = None;
        let mut __success = None;
        for __attempt in 0u32..($attempts as u32) {
            match $body {
                Ok(val) => {
                    __success = Some(val);
                    break;
                }
                Err(e) => {
                    __last_err = Some(e);
                    if __attempt + 1 < ($attempts as u32) {
                        ::tokio::time::sleep($crate::retry::backoff(
                            $delay as u64,
                            __attempt,
                        ))
                        .await;
                    }
                }
            }
        }
        match __success {
            Some(val) => Ok(val),
            None => Err(__last_err.expect("retry!: no attempts made")),
        }
    }};

    // attempts only
    (attempts: $attempts:expr, $body:block) => {
        $crate::retry!(attempts: $attempts, delay: 500, $body)
    };

    // delay only
    (delay: $delay:expr, $body:block) => {
        $crate::retry!(attempts: 3, delay: $delay, $body)
    };

    // just a block — use defaults
    ($body:block) => {
        $crate::retry!(attempts: 3, delay: 500, $body)
    };
}
