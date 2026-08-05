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
    async fn reload(&self, ctx: &AppContext) {
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
                // call, so nothing else will set it.
                self.inner
                    .dirty
                    .store(true, Ordering::Release);
                return;
            }
        };
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
            self.reload(&ctx)
                .await;

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

                if self
                    .inner
                    .dirty
                    .swap(false, Ordering::AcqRel)
                {
                    self.reload(&ctx)
                        .await;
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

/// Whether an operator-supplied template parses, for write-time validation. The
/// error is handlebars' own, derived from the operator's text and nothing else
/// — no remote response, no URL — so it is safe to return over the API.
pub fn validate_template(template: &str) -> Result<(), handlebars::TemplateError> {
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
