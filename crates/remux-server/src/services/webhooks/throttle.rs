//! Rate-limiting for the two per-event webhook warnings.
//!
//! Both of them are driven by something the server does not control. The
//! saturation warning in [`super::sender`] fires once per delivery past a
//! hook's ceiling of four in flight, and the emission site behind it —
//! `AuthenticationFailure` — is reachable **without credentials**: at 1000
//! failed logins a second that is 1000 `warn!` lines a second onto the
//! operator's disk, chosen by the attacker. The render-failure warning in
//! [`super::mod`] fires once per event for a template that does not render, so
//! a hook subscribed to `PlaybackProgress` with a broken template logs once per
//! progress tick, forever.
//!
//! Neither line is worth dropping — the first time each happens is exactly what
//! an operator needs to see. What is worth dropping is the repetition, so a
//! [`LogThrottle`] emits one line per key per window and carries the count of
//! what it suppressed since the last one.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// One log line per key per window.
///
/// Keyed by webhook id, so a flood against one hook never silences another —
/// the same reasoning as `sender::DeliverySlots`, and the same accepted leak:
/// entries are not pruned, which costs tens of bytes per hook an operator has
/// ever created.
pub(crate) struct LogThrottle {
    window: Duration,
    state: Mutex<HashMap<Uuid, Entry>>,
}

struct Entry {
    /// When the last line was emitted for this key.
    last: Instant,
    /// Occurrences swallowed since then.
    suppressed: u64,
}

impl LogThrottle {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            window,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(suppressed_since_the_last_line)` when the caller should log, and
    /// `None` when it should stay quiet. The first occurrence for a key always
    /// logs, with a count of zero.
    pub(crate) fn allow(&self, key: Uuid) -> Option<u64> {
        let now = Instant::now();
        // Short, await-free critical section. A poisoned lock is recovered
        // rather than propagated: a panic elsewhere must not turn logging into
        // a second failure.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.get_mut(&key) {
            None => {
                state.insert(
                    key,
                    Entry {
                        last: now,
                        suppressed: 0,
                    },
                );
                Some(0)
            }
            Some(entry) if now.duration_since(entry.last) >= self.window => {
                let suppressed = entry.suppressed;
                entry.last = now;
                entry.suppressed = 0;
                Some(suppressed)
            }
            Some(entry) => {
                entry.suppressed = entry
                    .suppressed
                    .saturating_add(1);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// The point of the whole module: an unauthenticated caller can drive the
    /// call site at whatever rate it manages, and only the first line lands.
    #[test]
    fn only_the_first_occurrence_in_a_window_logs() {
        let throttle = LogThrottle::new(Duration::from_secs(3600));
        assert_eq!(
            throttle.allow(key(1)),
            Some(0),
            "the first occurrence must always be logged"
        );
        for _ in 0..1000 {
            assert_eq!(throttle.allow(key(1)), None);
        }
    }

    /// A flood against one hook must not silence a different one — the same
    /// reason delivery slots are counted per hook.
    #[test]
    fn keys_are_throttled_independently() {
        let throttle = LogThrottle::new(Duration::from_secs(3600));
        assert_eq!(throttle.allow(key(1)), Some(0));
        assert_eq!(throttle.allow(key(1)), None);
        assert_eq!(
            throttle.allow(key(2)),
            Some(0),
            "another hook's first occurrence must still be logged"
        );
    }

    /// The line that does get through has to say how much it stands for, or the
    /// operator reads one dropped event where there were a thousand.
    #[test]
    fn the_next_line_carries_what_was_suppressed() {
        let throttle = LogThrottle::new(Duration::from_millis(30));
        assert_eq!(throttle.allow(key(1)), Some(0));
        for _ in 0..5 {
            assert_eq!(throttle.allow(key(1)), None);
        }
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(
            throttle.allow(key(1)),
            Some(5),
            "the line that gets through must carry the five it stands for"
        );
        // …and the counter restarts from there.
        assert_eq!(throttle.allow(key(1)), None);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(throttle.allow(key(1)), Some(1));
    }
}
