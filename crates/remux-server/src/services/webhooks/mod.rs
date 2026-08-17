//! Outgoing webhooks: an in-process event bus plus the background dispatcher
//! that turns events into HTTP deliveries.
//!
//! Emission is fire-and-forget (`WebhookService::emit`) so no request handler
//! ever waits on a webhook. A single dispatcher task owns the receiver and
//! keeps a cached snapshot of the enabled webhooks.

pub mod events;
mod payload;
mod sender;
mod template;
mod throttle;

pub use events::{
    DeviceEventData, PlaybackEventData, UserDataSaveReason, UserEventData, WebhookEvent,
};

use crate::{AppContext, db};
use remux_sdks::remux::{NotificationType, WebhookTestResult};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};
use tokio::{
    sync::{RwLock, broadcast, broadcast::error::RecvError},
    task::JoinHandle,
};
use tracing::{debug, warn};

/// Buffered events per subscriber. Large enough that a slow dispatcher pass
/// (one enrichment round-trip) never drops events under normal playback load.
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// How hard a [`Pacer`] tries to let the dispatcher catch up. Extracted so tests
/// do not have to wait out the real thresholds.
#[derive(Debug, Clone, Copy)]
struct PacingPolicy {
    /// Backlog above which emission waits.
    high_water: usize,
    /// Longest a single emission waits before going ahead anyway.
    max_wait: std::time::Duration,
    /// Gap between backlog checks.
    poll_interval: std::time::Duration,
    /// Consecutive exhausted waits after which the burst stops pacing. See
    /// [`Pacer`].
    give_up_after: u32,
}

impl Default for PacingPolicy {
    fn default() -> Self {
        Self {
            // Half the channel, so a paced burst keeps a margin for the unpaced
            // events (playback, auth) that share it.
            high_water: EVENT_CHANNEL_CAPACITY / 2,
            max_wait: std::time::Duration::from_secs(5),
            poll_interval: std::time::Duration::from_millis(50),
            give_up_after: 3,
        }
    }
}

/// Emits a burst of events at a rate the dispatcher can keep up with.
///
/// A library scan produces `ItemAdded` in tens of thousands, faster than the
/// dispatcher — one enrichment query per event — drains them, so unpaced the
/// overflow becomes a `Lagged` line and the "new movie" notification is silently
/// lost. Batching would fix the throughput but not the contract: one event per
/// item is what the plugin's templates are written against.
///
/// Two bounds stop a sick dispatcher from stalling the scan: a wait gives up
/// after `max_wait` and emits anyway, and `give_up_after` exhausted waits in a
/// row drop the burst back to unpaced emission — dropped events at full speed
/// beats `max_wait` per item for a whole library. Pacing resumes once the
/// backlog is back under the mark, so an early hiccup does not cost the rest.
///
/// One per burst: the give-up counter is what makes the second bound work.
pub struct Pacer {
    service: WebhookService,
    policy: PacingPolicy,
    consecutive_timeouts: u32,
    /// Latched: a burst that gave up does not start pacing again.
    gave_up: bool,
}

impl Pacer {
    fn new(service: WebhookService, policy: PacingPolicy) -> Self {
        Self {
            service,
            policy,
            consecutive_timeouts: 0,
            gave_up: false,
        }
    }

    /// Emit `event`, waiting first if the dispatcher is behind.
    pub async fn emit(&mut self, event: WebhookEvent) {
        // One channel read, no wait: a cleared stall resumes pacing, a dispatcher
        // that really is wedged stays given up on.
        if self.gave_up
            && self
                .service
                .tx
                .len()
                <= self
                    .policy
                    .high_water
        {
            self.gave_up = false;
            self.consecutive_timeouts = 0;
        }
        if !self.gave_up {
            self.wait_for_room()
                .await;
        }
        self.service
            .emit(event);
    }

    async fn wait_for_room(&mut self) {
        let deadline = std::time::Instant::now()
            + self
                .policy
                .max_wait;
        while self
            .service
            .tx
            .len()
            > self
                .policy
                .high_water
        {
            if std::time::Instant::now() >= deadline {
                self.consecutive_timeouts += 1;
                if self.consecutive_timeouts
                    >= self
                        .policy
                        .give_up_after
                {
                    warn!(
                        waits = self.consecutive_timeouts,
                        "webhook dispatcher is not draining, emitting the rest of \
                         this burst unpaced"
                    );
                    self.gave_up = true;
                }
                return;
            }
            tokio::time::sleep(
                self.policy
                    .poll_interval,
            )
            .await;
        }
        // Under the mark within the budget — the normal case is not waiting at all.
        self.consecutive_timeouts = 0;
    }
}

/// `Name` seen by the template of the synthetic event [`deliver_test`] sends.
pub const TEST_EVENT_TITLE: &str = "Test notification";

/// How often a hook may repeat its "template render failed" line. The failure
/// is per *event*, so an unthrottled line repeats for every progress tick.
const RENDER_FAILURE_WARN_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(60);

static RENDER_FAILURE_WARNINGS: std::sync::LazyLock<throttle::LogThrottle> =
    std::sync::LazyLock::new(|| throttle::LogThrottle::new(RENDER_FAILURE_WARN_WINDOW));

/// The enabled webhooks as last read from the database, plus everything that
/// would otherwise be recomputed per event.
pub(crate) struct LoadedWebhooks {
    pub hooks: Vec<db::Webhook>,
    /// Every hook's template, pre-compiled under its id, plus the custom helpers.
    pub registry: handlebars::Handlebars<'static>,
    /// Union of every enabled hook's subscriptions — the dispatcher fast-path.
    pub wanted: HashSet<NotificationType>,
    /// Server name/url/version as of this snapshot. Reloaded with the hooks so
    /// that renaming the server does not leave stale values in every payload.
    pub server: payload::ServerInfo,
}

/// The empty snapshot the dispatcher starts from. It must carry the helpers
/// too: a hook whose template uses one would otherwise fail to render until the
/// first reload.
impl Default for LoadedWebhooks {
    fn default() -> Self {
        Self {
            hooks: Vec::new(),
            registry: template::fresh_registry(),
            wanted: HashSet::new(),
            server: payload::ServerInfo::default(),
        }
    }
}

struct Inner {
    /// Set by the webhook CRUD endpoints; consumed by the dispatcher.
    dirty: AtomicBool,
    cache: RwLock<LoadedWebhooks>,
    /// `cache.wanted` as a bitmask, readable without awaiting a lock. This is
    /// what [`WebhookService::wants`] probes; see the note there.
    wanted_mask: AtomicU32,
}

/// One bit per [`NotificationType`], indexed by its discriminant. `None` for a
/// type too wide for the mask, which [`WebhookService::wants`] then answers
/// optimistically — wasted work, never a lost event.
fn wanted_bit(notification_type: NotificationType) -> Option<u32> {
    1u32.checked_shl(notification_type as u32)
}

/// Outgrowing the mask degrades silently — the probe would answer "always true"
/// past the 32nd variant — so make it a build error instead.
const _: () = assert!(
    <NotificationType as strum::EnumCount>::COUNT <= u32::BITS as usize,
    "NotificationType has outgrown the u32 `wants` mask — widen it to u64"
);

/// Whether [`WebhookService::reload`] got a snapshot out of the database.
///
/// A failure leaves `dirty` set, so the next event would retry the query — one
/// failing query per event for as long as the database is down, at a rate an
/// unauthenticated caller can drive through `AuthenticationFailure`. Hence
/// [`ReloadRetry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadOutcome {
    Loaded,
    Failed,
}

/// Base delay of the reload retry, grown exponentially per consecutive failure.
const RELOAD_RETRY_BASE_MS: u64 = 500;

/// Cap on consecutive failures counted, so the delay tops out at
/// `RELOAD_RETRY_BASE_MS * 2^4` ≈ 8s rather than growing without bound.
///
/// Kept low because nothing interrupts the wait: it is also how long a recovered
/// database goes unnoticed, and how long `dirty` stays raised — during which
/// [`WebhookService::wants`] answers `true` for everything.
const RELOAD_RETRY_MAX_EXPONENT: u32 = 4;

/// When the dispatcher may next attempt a reload.
///
/// Owned by the dispatcher task, which is the only caller of
/// [`WebhookService::reload`] — see the invariant documented there.
#[derive(Debug, Default)]
struct ReloadRetry {
    /// `None` once a reload has succeeded: the next one runs immediately.
    not_before: Option<std::time::Instant>,
    consecutive_failures: u32,
}

impl ReloadRetry {
    fn is_due(&self) -> bool {
        match self.not_before {
            Some(deadline) => std::time::Instant::now() >= deadline,
            None => true,
        }
    }

    fn record(&mut self, outcome: ReloadOutcome) {
        match outcome {
            ReloadOutcome::Loaded => {
                self.not_before = None;
                self.consecutive_failures = 0;
            }
            ReloadOutcome::Failed => {
                let attempt = self
                    .consecutive_failures
                    .min(RELOAD_RETRY_MAX_EXPONENT);
                self.not_before = Some(
                    std::time::Instant::now()
                        + remux_utils::retry::backoff(RELOAD_RETRY_BASE_MS, attempt),
                );
                self.consecutive_failures = self
                    .consecutive_failures
                    .saturating_add(1);
            }
        }
    }
}

#[derive(Clone)]
pub struct WebhookService {
    tx: broadcast::Sender<Arc<WebhookEvent>>,
    inner: Arc<Inner>,
}

impl WebhookService {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            tx,
            inner: Arc::new(Inner {
                // The dispatcher loads the cache once on startup, so nothing is
                // stale until the CRUD endpoints say so.
                dirty: AtomicBool::new(false),
                cache: RwLock::new(LoadedWebhooks::default()),
                // Everything is "wanted" until the first reload has run:
                // skipping is only correct against a loaded snapshot.
                wanted_mask: AtomicU32::new(u32::MAX),
            }),
        }
    }

    /// Publish an event. Never blocks and never fails the caller: with no
    /// dispatcher running, or a lagging one, the event is dropped.
    pub fn emit(&self, event: WebhookEvent) {
        let _ = self
            .tx
            .send(Arc::new(event));
    }

    /// A [`Pacer`] for one burst of events. See its documentation.
    pub fn pacer(&self) -> Pacer {
        Pacer::new(self.clone(), PacingPolicy::default())
    }

    /// Whether any enabled webhook subscribes to `notification_type`.
    ///
    /// `emit` is cheap, but building an event is not — cloning usernames and
    /// device names, and for `ItemDeleted` re-reading a whole [`db::Media`], per
    /// progress tick. Guard those sites with this.
    ///
    /// Lock-free (two atomic loads) and deliberately conservative: a pending
    /// reload, or a subscription set too wide for the mask, answers `true`. It
    /// is an optimisation, never the authority — the dispatcher re-checks every
    /// event against the real snapshot.
    ///
    /// `dirty` must be consulted alongside the mask: a mask narrowed from a
    /// snapshot that predates an `invalidate` would otherwise suppress exactly
    /// the guarded events that wake the dispatcher into consuming the flag,
    /// leaving the staleness with nothing to heal it.
    pub fn wants(&self, notification_type: NotificationType) -> bool {
        let Some(bit) = wanted_bit(notification_type) else {
            return true;
        };
        self.inner
            .dirty
            .load(Ordering::Acquire)
            || self
                .inner
                .wanted_mask
                .load(Ordering::Relaxed)
                & bit
                != 0
    }

    /// Mark the cached webhook set stale. The dispatcher reloads before it
    /// handles the next event.
    pub fn invalidate(&self) {
        // Widened before the flag is raised: a hook that just gained a
        // subscription must not have its events skipped in the window before
        // the dispatcher reloads.
        self.inner
            .wanted_mask
            .store(u32::MAX, Ordering::Relaxed);
        self.inner
            .dirty
            .store(true, Ordering::Release);
    }

    /// Replace the cached snapshot from the database. On error the previous
    /// hook set is kept and the cache is marked stale again — a transient DB
    /// failure must not silently disable every webhook. Nothing else re-raises
    /// the flag, so a failure that left it down would never be retried.
    ///
    /// The server identity is reloaded here too, which is why settings writers
    /// call [`Self::invalidate`]: a rename would otherwise ship the old name in
    /// every payload until restart.
    ///
    /// Invariant: this is the only writer of `cache`, and it is only ever
    /// called from the dispatcher task itself, at a point where that task
    /// holds no read guard. That is what makes it safe for the dispatcher to
    /// hold the read guard across `enrich_item().await` — no other task can be
    /// waiting for the write lock.
    async fn reload(&self, ctx: &AppContext) -> ReloadOutcome {
        // Never fails (falls back to defaults), so it is applied even when the
        // hook query below does not.
        let server = payload::ServerInfo::load(ctx).await;

        let hooks = match db::Webhook::get_enabled(&ctx.db).await {
            Ok(hooks) => hooks,
            Err(e) => {
                warn!(error = %e, "failed to load webhooks, keeping previous set");
                self.inner
                    .cache
                    .write()
                    .await
                    .server = server;
                // Ask for another attempt: the flag was consumed before the
                // call, so nothing else will set it. The caller decides *when*
                // that attempt happens — see [`ReloadOutcome`].
                self.inner
                    .dirty
                    .store(true, Ordering::Release);
                return ReloadOutcome::Failed;
            }
        };

        // Slots are keyed by hook id and created on first delivery, so this is
        // the only place that ever learns a hook is gone.
        sender::retain_delivery_slots(
            &hooks
                .iter()
                .map(|hook| hook.id)
                .collect(),
        );
        let wanted: HashSet<NotificationType> = hooks
            .iter()
            .flat_map(|hook| {
                hook.notification_types
                    .iter()
                    .copied()
            })
            .collect();
        let mask = wanted
            .iter()
            .filter_map(|t| wanted_bit(*t))
            .fold(0u32, |mask, bit| mask | bit);
        let mut cache = self
            .inner
            .cache
            .write()
            .await;
        *cache = LoadedWebhooks {
            registry: template::build_registry(&hooks),
            hooks,
            wanted,
            server,
        };
        // Only narrow when the flag is down: finding it set again means `hooks`
        // predates an `invalidate` whose widening this store would clobber.
        if !self
            .inner
            .dirty
            .load(Ordering::Acquire)
        {
            self.inner
                .wanted_mask
                .store(mask, Ordering::Relaxed);
        }
        ReloadOutcome::Loaded
    }

    /// Whether `hook` wants `event`. `item_kind` is `None` when the event
    /// carries no item.
    pub(crate) fn matches(
        hook: &db::Webhook,
        event: &WebhookEvent,
        item_kind: Option<&db::MediaKind>,
    ) -> bool {
        // 1. Subscription. An empty list matches nothing, mirroring the plugin.
        if !hook
            .notification_types
            .contains(&event.notification_type())
        {
            return false;
        }

        // 2. User filter. Empty means every user; events with no user are exempt.
        if !hook
            .user_filter
            .is_empty()
            && let Some(user_id) = event.user_id()
            && !hook
                .user_filter
                .contains(&user_id)
        {
            return false;
        }

        // 3. Item-type toggles, only for events that carry an item.
        if let Some(kind) = item_kind {
            let types = &hook.item_types;
            let allowed = match kind {
                db::MediaKind::Movie => types.movies,
                db::MediaKind::Episode => types.episodes,
                db::MediaKind::Series => types.series,
                db::MediaKind::Season => types.seasons,
                db::MediaKind::Album => types.albums,
                db::MediaKind::Track => types.songs,
                _ => types.videos,
            };
            if !allowed {
                return false;
            }
        }

        true
    }

    /// Run the dispatcher for the lifetime of the process. Owns the only
    /// receiver.
    ///
    /// The receiver is created here rather than inside the task: a broadcast
    /// channel drops sends made with no subscriber, and `init_app` starts
    /// emitting before the spawned task gets its first poll.
    pub fn spawn_dispatcher(self, ctx: AppContext) -> JoinHandle<()> {
        let mut rx = self
            .tx
            .subscribe();
        tokio::spawn(async move {
            // Local to the task on purpose: `reload` is only ever called from
            // here, so this needs no synchronisation.
            let mut retry = ReloadRetry::default();
            retry.record(
                self.reload(&ctx)
                    .await,
            );

            loop {
                let event = match rx
                    .recv()
                    .await
                {
                    Ok(event) => event,
                    Err(RecvError::Lagged(dropped)) => {
                        warn!(dropped, "webhook dispatcher lagged");
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                };

                // Only consume the flag when a reload is due: a failed reload
                // re-raises it, so without the deadline check that would be one
                // failing query per event.
                if retry.is_due()
                    && self
                        .inner
                        .dirty
                        .swap(false, Ordering::AcqRel)
                {
                    retry.record(
                        self.reload(&ctx)
                            .await,
                    );
                }

                let cache = self
                    .inner
                    .cache
                    .read()
                    .await;
                if !cache
                    .wanted
                    .contains(&event.notification_type())
                {
                    continue;
                }

                let item = payload::enrich_item(&ctx, &event).await;
                // An unresolved item must not be delivered: `matches` skips the
                // item-type rule without a kind, so a hook with every type
                // unticked would fire. `ItemDeleted` carries its row inline and
                // never lands here.
                if event
                    .item_id()
                    .is_some()
                    && item.is_none()
                {
                    // `enrich_item` already logged the cause at warn, and a
                    // scan deleting rows behind an in-flight event is expected.
                    debug!(
                        notification_type = %event.notification_type(),
                        "webhook event dropped, its item could not be resolved"
                    );
                    continue;
                }
                let item_kind = item
                    .as_ref()
                    .map(|i| {
                        &i.media
                            .kind
                    });
                let targets: Vec<&db::Webhook> = cache
                    .hooks
                    .iter()
                    .filter(|hook| Self::matches(hook, &event, item_kind))
                    .collect();
                if targets.is_empty() {
                    continue;
                }

                // Built once per event; `render` applies the per-hook overlay.
                let data = payload::build_data(&cache.server, &event, item.as_ref());
                for hook in targets {
                    match template::render(hook, &cache.registry, &data) {
                        // Spawned so one slow endpoint cannot stall the
                        // dispatcher, and bounded so a dead one cannot grow
                        // tasks without limit.
                        Ok(Some(body)) => {
                            sender::spawn_delivery(hook.clone(), body);
                        }
                        // `skip_empty_message_body` suppressed the delivery.
                        Ok(None) => {}
                        Err(e) => {
                            if let Some(suppressed) =
                                RENDER_FAILURE_WARNINGS.allow(hook.id)
                            {
                                warn!(
                                    webhook = %hook.name,
                                    webhook_id = %hook.id,
                                    error = %e,
                                    suppressed,
                                    "webhook template render failed"
                                );
                            }
                        }
                    }
                }
            }
        })
    }
}

// --- the admin "test this webhook" path --------------------------------------

/// Whether an operator-supplied template parses **and renders**, for write-time
/// validation. The error is handlebars' own, derived from the operator's text
/// and nothing else — no remote response, no URL — so it is safe to return over
/// the API.
pub fn validate_template(template: &str) -> anyhow::Result<()> {
    template::validate(template)
}

/// Render `hook`'s body for the synthetic test event.
///
/// The template is compiled here rather than taken from the dispatcher's cached
/// registry, which only reloads when the dispatcher next sees an event — the
/// hook being tested was very likely saved a moment ago.
///
/// Through [`template::single_registry`], not `build_registry`: the latter
/// warns-and-skips an unparseable template, so `render` would fail with
/// "Template not found: <uuid>" instead of the operator's own syntax error.
fn test_body(
    server: &payload::ServerInfo,
    hook: &db::Webhook,
) -> anyhow::Result<Option<String>> {
    let event = WebhookEvent::Generic {
        title: TEST_EVENT_TITLE.to_string(),
        extra: Vec::new(),
    };
    let data = payload::build_data(server, &event, None);
    let registry = template::single_registry(hook)?;
    template::render(hook, &registry, &data)
}

/// Deliver one synthetic `Generic` event to `hook` and report what happened.
///
/// Deliberately not routed through [`WebhookService::emit`]: a hook that is
/// disabled, or subscribes to nothing, must still be testable, and the
/// fire-and-forget path cannot answer "did *this* webhook work?".
///
/// One attempt, no retry, and the answer handed straight back to the caller.
pub async fn deliver_test(ctx: &AppContext, hook: &db::Webhook) -> WebhookTestResult {
    let server = payload::ServerInfo::load(ctx).await;
    match test_body(&server, hook) {
        Ok(Some(body)) => sender::send_test(hook, &body).await,
        // `skip_empty_message_body` would drop this delivery in production.
        Ok(None) => WebhookTestResult {
            success: false,
            status_code: None,
            error: Some(
                "the template rendered an empty body and this webhook skips empty bodies"
                    .to_string(),
            ),
        },
        Err(e) => WebhookTestResult {
            success: false,
            status_code: None,
            error: Some(format!("template render failed: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use remux_sdks::remux::{DiscordMentionType, WebhookDestination, WebhookItemTypes};
    use uuid::Uuid;

    // --- reload pacing -----------------------------------------------------

    #[test]
    fn a_fresh_reload_retry_is_due_immediately() {
        assert!(ReloadRetry::default().is_due());
    }

    #[test]
    fn a_failed_reload_is_not_retried_until_its_deadline() {
        let mut retry = ReloadRetry::default();
        retry.record(ReloadOutcome::Failed);
        assert!(
            !retry.is_due(),
            "a failing DB must not be re-queried on the very next event"
        );
        assert_eq!(retry.consecutive_failures, 1);
    }

    /// The delay has to grow, otherwise a long outage is still one query per
    /// event once the first short delay has elapsed.
    #[test]
    fn consecutive_failures_push_the_deadline_further_out() {
        let mut retry = ReloadRetry::default();
        retry.record(ReloadOutcome::Failed);
        let first = retry
            .not_before
            .expect("a failure sets a deadline");
        for _ in 0..4 {
            retry.record(ReloadOutcome::Failed);
        }
        assert!(
            retry
                .not_before
                .expect("still set")
                > first,
            "the deadline must move out as failures accumulate"
        );
    }

    #[test]
    fn a_successful_reload_clears_the_pacing() {
        let mut retry = ReloadRetry::default();
        retry.record(ReloadOutcome::Failed);
        retry.record(ReloadOutcome::Loaded);
        assert!(retry.is_due(), "a recovered DB must be readable at once");
        assert_eq!(retry.consecutive_failures, 0);
    }

    // --- pacing ------------------------------------------------------------

    fn test_event() -> WebhookEvent {
        WebhookEvent::Generic {
            title: "t".into(),
            extra: Vec::new(),
        }
    }

    /// A policy whose waits are short enough to sit in a unit test, with enough
    /// slack that the parallel suite cannot flake it.
    const FAST_PACING: PacingPolicy = PacingPolicy {
        high_water: 2,
        max_wait: std::time::Duration::from_millis(500),
        poll_interval: std::time::Duration::from_millis(10),
        give_up_after: 2,
    };

    fn fast_pacer(service: &WebhookService) -> Pacer {
        Pacer::new(service.clone(), FAST_PACING)
    }

    /// Fills the channel past the high-water mark and keeps a receiver that
    /// never drains, so the backlog only ever grows.
    fn wedged(service: &WebhookService) -> broadcast::Receiver<Arc<WebhookEvent>> {
        let rx = service
            .tx
            .subscribe();
        for _ in 0..=FAST_PACING.high_water {
            service.emit(test_event());
        }
        rx
    }

    #[tokio::test]
    async fn pacing_does_not_wait_while_the_backlog_is_low() {
        let service = WebhookService::new();
        let _rx = service
            .tx
            .subscribe();
        let started = std::time::Instant::now();
        fast_pacer(&service)
            .emit(test_event())
            .await;
        assert!(
            started.elapsed() < FAST_PACING.max_wait,
            "an idle dispatcher must not slow emission down"
        );
        assert_eq!(
            service
                .tx
                .len(),
            1
        );
    }

    /// The wait is bounded, so a dispatcher that never drains must not stall a
    /// scan for good.
    #[tokio::test]
    async fn pacing_emits_anyway_once_the_wait_runs_out() {
        let service = WebhookService::new();
        let _rx = wedged(&service);

        let started = std::time::Instant::now();
        fast_pacer(&service)
            .emit(test_event())
            .await;

        assert!(
            started.elapsed() >= FAST_PACING.max_wait,
            "it must actually have paced"
        );
        assert_eq!(
            service
                .tx
                .len(),
            FAST_PACING.high_water + 2,
            "past the deadline the event goes out anyway rather than hanging"
        );
    }

    /// Otherwise a wedged dispatcher would cost `max_wait` per item for the
    /// length of a library — worse than the dropped events it replaces.
    #[tokio::test]
    async fn pacing_gives_up_on_the_rest_of_a_burst_after_repeated_timeouts() {
        let service = WebhookService::new();
        let _rx = wedged(&service);
        let mut pacer = fast_pacer(&service);

        for _ in 0..FAST_PACING.give_up_after {
            pacer
                .emit(test_event())
                .await;
        }
        assert!(pacer.gave_up, "repeated timeouts must latch the give-up");

        let started = std::time::Instant::now();
        pacer
            .emit(test_event())
            .await;
        // Half the budget, not the poll interval: a tighter bound would measure
        // the test runner's scheduling rather than the pacer.
        assert!(
            started.elapsed() < FAST_PACING.max_wait / 2,
            "a burst that gave up must not pace again"
        );
    }

    /// A hiccup early in a scan must not cost the pacing for the rest of it.
    #[tokio::test]
    async fn pacing_resumes_after_a_stall_clears() {
        let service = WebhookService::new();
        let mut rx = wedged(&service);
        let mut pacer = fast_pacer(&service);
        for _ in 0..FAST_PACING.give_up_after {
            pacer
                .emit(test_event())
                .await;
        }
        assert!(pacer.gave_up, "the stall must have latched the give-up");

        while rx
            .try_recv()
            .is_ok()
        {}
        pacer
            .emit(test_event())
            .await;

        assert!(
            !pacer.gave_up,
            "a drained backlog must put the burst back under pacing"
        );
        assert_eq!(pacer.consecutive_timeouts, 0);
    }

    /// The pacing has to end as soon as the dispatcher drains, not at the
    /// deadline — and a drain must clear the give-up counter.
    #[tokio::test]
    async fn pacing_resumes_as_soon_as_the_backlog_drains() {
        let service = WebhookService::new();
        let mut rx = wedged(&service);

        let drainer = tokio::spawn(async move {
            tokio::time::sleep(FAST_PACING.poll_interval).await;
            while rx
                .try_recv()
                .is_ok()
            {}
            rx
        });

        let mut pacer = fast_pacer(&service);
        let started = std::time::Instant::now();
        pacer
            .emit(test_event())
            .await;
        let waited = started.elapsed();
        let _rx = drainer
            .await
            .expect("the drainer must not panic");

        assert!(
            waited < FAST_PACING.max_wait,
            "emission must resume on drain, not wait out the deadline: {waited:?}"
        );
        assert!(
            !pacer.gave_up,
            "a dispatcher that drains must not be given up on"
        );
        assert_eq!(pacer.consecutive_timeouts, 0);
    }

    const NONE_ENABLED: WebhookItemTypes = WebhookItemTypes {
        movies: false,
        episodes: false,
        series: false,
        seasons: false,
        albums: false,
        songs: false,
        videos: false,
    };

    const ALL_ENABLED: WebhookItemTypes = WebhookItemTypes {
        movies: true,
        episodes: true,
        series: true,
        seasons: true,
        albums: true,
        songs: true,
        videos: true,
    };

    fn hook(
        notification_types: Vec<NotificationType>,
        user_filter: Vec<Uuid>,
        item_types: WebhookItemTypes,
    ) -> db::Webhook {
        let now = Utc::now();
        db::Webhook {
            id: Uuid::from_u128(100),
            name: "test".into(),
            enabled: true,
            url: "https://example.test/hook".into(),
            template: "{{ItemName}}".into(),
            destination: WebhookDestination::Discord {
                avatar_url: None,
                bot_username: None,
                embed_color: None,
                mention_type: DiscordMentionType::None,
            },
            notification_types,
            user_filter,
            item_types,
            send_all_properties: false,
            trim_whitespace: false,
            skip_empty_message_body: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Subscribes to everything the tests emit, with both other filters wide open.
    fn permissive(notification_types: Vec<NotificationType>) -> db::Webhook {
        hook(notification_types, vec![], ALL_ENABLED)
    }

    fn item_added() -> WebhookEvent {
        WebhookEvent::ItemAdded {
            item_id: Uuid::from_u128(2),
        }
    }

    fn alice() -> Uuid {
        Uuid::from_u128(1)
    }

    fn playback_by(user_id: Uuid) -> WebhookEvent {
        WebhookEvent::PlaybackStart {
            playback: PlaybackEventData {
                user: UserEventData {
                    id: user_id,
                    username: "alice".into(),
                },
                item_id: Uuid::from_u128(2),
                device: DeviceEventData {
                    id: "device-1".into(),
                    name: "Living Room".into(),
                    client_name: "Jellyfin Web".into(),
                    remote_ip: None,
                },
                position_ticks: 0,
                is_paused: false,
                play_method: None,
            },
        }
    }

    // --- the test event ---------------------------------------------------

    fn test_server_info() -> payload::ServerInfo {
        payload::ServerInfo {
            id: "server-1".into(),
            name: "remux".into(),
            version: "0.0.0".into(),
            url: "https://remux.test".into(),
        }
    }

    #[test]
    fn the_test_event_renders_the_title_and_the_server_variables() {
        let hook = db::Webhook {
            template: r#"{"content":"{{Name}}","server":"{{ServerName}}","type":"{{NotificationType}}"}"#.into(),
            ..permissive(vec![NotificationType::Generic])
        };

        let body = test_body(&test_server_info(), &hook)
            .expect("the fixture template must render")
            .expect("a non-empty body must be produced");

        assert_eq!(
            body,
            r#"{"content":"Test notification","server":"remux","type":"Generic"}"#
        );
    }

    /// What comes back must be the *parse* error: `build_registry` would answer
    /// "Template not found: <uuid>", so `is_err()` alone is not enough here.
    #[test]
    fn a_template_that_does_not_compile_reports_its_parse_error() {
        let hook = db::Webhook {
            template: "{{#if_equals A}}unclosed".into(),
            ..permissive(vec![NotificationType::Generic])
        };
        let error = test_body(&test_server_info(), &hook)
            .expect_err("an uncompilable template must surface as an error")
            .to_string();
        assert!(
            !error.contains("Template not found"),
            "the operator must not be told their template is missing: {error}"
        );
        assert!(
            error.contains("{{#if_equals A}}unclosed"),
            "the parse error must quote the operator's own template: {error}"
        );
    }

    #[test]
    fn validate_template_rejects_a_template_that_does_not_parse() {
        assert!(validate_template(r#"{"content":"{{Name}}"}"#).is_ok());
        assert!(validate_template("").is_ok());
        let error = validate_template("{{#if_equals A}}unclosed")
            .expect_err("an unclosed block must be refused")
            .to_string();
        assert!(
            !error.is_empty() && !error.contains("Template not found"),
            "the refusal must carry the parse error: {error}"
        );
    }

    #[test]
    fn an_empty_body_is_reported_rather_than_posted() {
        let hook = db::Webhook {
            template: "   ".into(),
            skip_empty_message_body: true,
            ..permissive(vec![NotificationType::Generic])
        };
        assert_eq!(
            test_body(&test_server_info(), &hook).expect("rendering must succeed"),
            None
        );
    }

    // --- cached snapshot -------------------------------------------------

    /// The registry is built in two places (here and in `reload`); both must
    /// carry the custom helpers.
    #[test]
    fn the_default_snapshot_registry_carries_the_custom_helpers() {
        let snapshot = LoadedWebhooks::default();
        let body = snapshot
            .registry
            .render_template(
                "{{#if_equals A \"a\"}}ok{{/if_equals}}",
                &serde_json::json!({ "A": "A" }),
            )
            .expect("helpers must be registered on the default snapshot");
        assert_eq!(body, "ok");
    }

    /// The startup reload has nothing to "keep" and the flag has already been
    /// consumed, so a failure there is permanent unless it asks for a retry.
    #[tokio::test]
    async fn a_failed_reload_asks_for_another_attempt() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .expect("test server");
        let service = WebhookService::new();

        // The dispatcher's startup path: consume the flag, then load — except
        // the database is gone.
        service
            .inner
            .dirty
            .swap(false, Ordering::AcqRel);
        guard
            .0
            .db
            .close()
            .await;

        service
            .reload(&guard.0)
            .await;

        assert!(
            service
                .inner
                .dirty
                .load(Ordering::Acquire),
            "a failed load must leave the cache stale so the next event retries it"
        );
        assert!(
            service.wants(NotificationType::PlaybackProgress),
            "and the probe must stay open until a snapshot actually lands"
        );
    }

    /// Or the dispatcher would reload on every single event.
    #[tokio::test]
    async fn a_successful_reload_leaves_the_cache_clean() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .expect("test server");
        let service = WebhookService::new();

        service
            .inner
            .dirty
            .swap(false, Ordering::AcqRel);
        service
            .reload(&guard.0)
            .await;

        assert!(
            !service
                .inner
                .dirty
                .load(Ordering::Acquire),
            "a load that succeeded must not mark the cache stale"
        );
    }

    // --- the `wants` probe ------------------------------------------------

    /// The bit index is the enum's discriminant, which nothing else pins down:
    /// two types sharing a bit would make `wants` answer for the wrong one.
    #[test]
    fn every_notification_type_has_its_own_bit() {
        let types = [
            NotificationType::ItemAdded,
            NotificationType::ItemDeleted,
            NotificationType::Generic,
            NotificationType::PlaybackStart,
            NotificationType::PlaybackProgress,
            NotificationType::PlaybackStop,
            NotificationType::AuthenticationSuccess,
            NotificationType::AuthenticationFailure,
            NotificationType::SessionStart,
            NotificationType::TaskCompleted,
            NotificationType::UserCreated,
            NotificationType::UserDeleted,
            NotificationType::UserUpdated,
            NotificationType::UserPasswordChanged,
            NotificationType::UserDataSaved,
        ];
        let bits: HashSet<u32> = types
            .iter()
            .map(|t| {
                wanted_bit(*t).unwrap_or_else(|| panic!("{t} must fit in the mask"))
            })
            .collect();
        assert_eq!(
            bits.len(),
            types.len(),
            "each notification type must map to its own bit"
        );
    }

    /// Skipping is only ever correct against a snapshot known to be current, so
    /// the probe stays open before the first load and while a reload is pending.
    #[tokio::test]
    async fn the_probe_is_open_until_a_snapshot_says_otherwise() {
        let service = WebhookService::new();
        assert!(
            service.wants(NotificationType::PlaybackProgress),
            "a service whose snapshot has never loaded must want everything"
        );

        // Narrowed exactly as `reload` narrows it: nothing subscribes.
        service
            .inner
            .wanted_mask
            .store(0, Ordering::Relaxed);
        assert!(!service.wants(NotificationType::PlaybackProgress));

        service.invalidate();
        assert!(
            service.wants(NotificationType::PlaybackProgress),
            "invalidate must re-open the probe until the reload it asked for lands"
        );
    }

    /// `reload` publishes a mask derived from rows read some time earlier, and
    /// an `invalidate` landing in between must not have its widening clobbered:
    /// a closed probe suppresses the very events that would reopen it.
    #[tokio::test]
    async fn a_reload_that_races_an_invalidate_leaves_the_probe_open() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .expect("test server");
        let service = WebhookService::new();

        // The database holds no webhooks, so this reload computes an empty
        // mask.
        service.invalidate();
        service
            .inner
            .dirty
            .swap(false, Ordering::AcqRel);

        // Mid-read: the operator saves a hook that subscribes to a guarded event.
        service.invalidate();

        // The reload lands, carrying the snapshot from before that save.
        service
            .reload(&guard.0)
            .await;

        assert!(
            service.wants(NotificationType::PlaybackProgress),
            "a reload that raced an invalidate must not leave the probe closed \
             — the flag is still set, so its own snapshot is known to be stale"
        );
    }

    // --- rule 1: notification types -------------------------------------

    /// Deliberate parity with the Jellyfin webhook plugin, even with every
    /// other filter wide open.
    #[test]
    fn empty_notification_types_match_nothing() {
        let hook = hook(vec![], vec![], ALL_ENABLED);
        assert!(!WebhookService::matches(&hook, &item_added(), None));
        assert!(!WebhookService::matches(
            &hook,
            &item_added(),
            Some(&db::MediaKind::Movie)
        ));
        assert!(!WebhookService::matches(&hook, &playback_by(alice()), None));
    }

    #[test]
    fn only_subscribed_notification_types_match() {
        let hook = permissive(vec![NotificationType::PlaybackStart]);
        assert!(
            WebhookService::matches(&hook, &playback_by(alice()), None),
            "subscribed type must match"
        );
        assert!(
            !WebhookService::matches(&hook, &item_added(), None),
            "unsubscribed type must not match"
        );
    }

    // --- rule 2: user filter --------------------------------------------

    #[test]
    fn empty_user_filter_accepts_any_user() {
        let hook = permissive(vec![NotificationType::PlaybackStart]);
        assert!(WebhookService::matches(&hook, &playback_by(alice()), None));
        assert!(WebhookService::matches(
            &hook,
            &playback_by(Uuid::from_u128(99)),
            None
        ));
    }

    #[test]
    fn user_filter_accepts_only_listed_users() {
        let hook = hook(
            vec![NotificationType::PlaybackStart],
            vec![alice(), Uuid::from_u128(7)],
            ALL_ENABLED,
        );
        assert!(
            WebhookService::matches(&hook, &playback_by(alice()), None),
            "listed user must match"
        );
        assert!(
            WebhookService::matches(&hook, &playback_by(Uuid::from_u128(7)), None),
            "any listed user must match"
        );
        assert!(
            !WebhookService::matches(&hook, &playback_by(Uuid::from_u128(99)), None),
            "unlisted user must not match"
        );
    }

    #[test]
    fn user_filter_is_ignored_for_events_without_a_user() {
        let hook = hook(
            vec![NotificationType::ItemAdded],
            vec![alice()],
            ALL_ENABLED,
        );
        assert!(
            WebhookService::matches(&hook, &item_added(), None),
            "an event with no user must not be filtered out by user_filter"
        );
    }

    // --- rule 3: item types ---------------------------------------------

    /// `(media kind, the single `item_types` flag that gates it)`.
    fn item_type_cases() -> Vec<(db::MediaKind, WebhookItemTypes)> {
        vec![
            (
                db::MediaKind::Movie,
                WebhookItemTypes {
                    movies: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Episode,
                WebhookItemTypes {
                    episodes: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Series,
                WebhookItemTypes {
                    series: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Season,
                WebhookItemTypes {
                    seasons: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Album,
                WebhookItemTypes {
                    albums: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Track,
                WebhookItemTypes {
                    songs: true,
                    ..NONE_ENABLED
                },
            ),
            // Fallthrough: everything not named above is gated by `videos`.
            (
                db::MediaKind::Artist,
                WebhookItemTypes {
                    videos: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::Collection,
                WebhookItemTypes {
                    videos: true,
                    ..NONE_ENABLED
                },
            ),
            (
                db::MediaKind::TvChannel,
                WebhookItemTypes {
                    videos: true,
                    ..NONE_ENABLED
                },
            ),
        ]
    }

    fn inverted(types: &WebhookItemTypes) -> WebhookItemTypes {
        WebhookItemTypes {
            movies: !types.movies,
            episodes: !types.episodes,
            series: !types.series,
            seasons: !types.seasons,
            albums: !types.albums,
            songs: !types.songs,
            videos: !types.videos,
        }
    }

    /// Both halves are needed to pin the mapping: enabling only that flag must
    /// match, and disabling only that flag must not.
    #[test]
    fn each_media_kind_is_gated_by_its_own_flag() {
        for (kind, only_this) in item_type_cases() {
            let enabled = permissive(vec![NotificationType::ItemAdded]);
            let enabled = db::Webhook {
                item_types: only_this.clone(),
                ..enabled
            };
            assert!(
                WebhookService::matches(&enabled, &item_added(), Some(&kind)),
                "{kind:?} must match when only its own flag is enabled ({only_this:?})"
            );

            let all_but_this = inverted(&only_this);
            let disabled = db::Webhook {
                item_types: all_but_this.clone(),
                ..permissive(vec![NotificationType::ItemAdded])
            };
            assert!(
                !WebhookService::matches(&disabled, &item_added(), Some(&kind)),
                "{kind:?} must not match when only its own flag is disabled ({all_but_this:?})"
            );
        }
    }

    #[test]
    fn item_type_flags_are_ignored_when_the_event_has_no_item() {
        let hook = hook(vec![NotificationType::ItemAdded], vec![], NONE_ENABLED);
        assert!(
            WebhookService::matches(&hook, &item_added(), None),
            "with no item kind in hand, item_types must not gate the event"
        );
    }

    // --- the three rules are ANDed ---------------------------------------

    #[test]
    fn all_three_rules_must_pass() {
        let base = hook(
            vec![NotificationType::PlaybackStart],
            vec![alice()],
            WebhookItemTypes {
                movies: true,
                ..NONE_ENABLED
            },
        );
        let event = playback_by(alice());
        assert!(WebhookService::matches(
            &base,
            &event,
            Some(&db::MediaKind::Movie)
        ));

        // Break exactly one rule at a time.
        assert!(!WebhookService::matches(
            &db::Webhook {
                notification_types: vec![NotificationType::PlaybackStop],
                ..base.clone()
            },
            &event,
            Some(&db::MediaKind::Movie)
        ));
        assert!(!WebhookService::matches(
            &db::Webhook {
                user_filter: vec![Uuid::from_u128(99)],
                ..base.clone()
            },
            &event,
            Some(&db::MediaKind::Movie)
        ));
        assert!(!WebhookService::matches(
            &base,
            &event,
            Some(&db::MediaKind::Episode)
        ));
    }
}
