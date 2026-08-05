//! Outgoing webhooks: an in-process event bus plus the background dispatcher
//! that turns events into HTTP deliveries.
//!
//! Emission is fire-and-forget (`WebhookService::emit`) so no request handler
//! ever waits on a webhook. A single dispatcher task owns the receiver, keeps a
//! cached snapshot of the enabled webhooks, and fans each event out to the
//! hooks that match it.

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

/// How often a hook may repeat its "template render failed" line.
///
/// The failure is per *event*, so a hook subscribed to `PlaybackProgress` with
/// a template that does not render logs once per progress tick, forever. The
/// first line is what an operator needs; the rest is the same line again.
const RENDER_FAILURE_WARN_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(60);

static RENDER_FAILURE_WARNINGS: std::sync::LazyLock<throttle::LogThrottle> =
    std::sync::LazyLock::new(|| throttle::LogThrottle::new(RENDER_FAILURE_WARN_WINDOW));

/// The enabled webhooks as last read from the database, plus everything derived
/// from them that would otherwise be recomputed per event.
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

/// One bit per [`NotificationType`], indexed by its discriminant (the enum is
/// fieldless). `None` for a type that would not fit in the mask — see
/// [`WebhookService::wants`], which answers those optimistically, so an enum
/// too wide for the mask costs wasted work but never a lost event.
fn wanted_bit(notification_type: NotificationType) -> Option<u32> {
    1u32.checked_shl(notification_type as u32)
}

/// The degradation above is correct but *silent*: a 33rd variant would quietly
/// turn the probe into "always true" for everything past the 32nd, and no test
/// would notice. Make outgrowing the mask a build error instead, so the choice
/// (widen the mask, or accept the loss) is made deliberately.
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
                // Everything is "wanted" until the first reload has run: the
                // dispatcher buffers the events emitted during startup and
                // filters them properly once its snapshot is loaded, so the
                // probe must not tell callers to skip building them.
                wanted_mask: AtomicU32::new(u32::MAX),
            }),
        }
    }

    /// Publish an event. Never blocks and never fails the caller: with no
    /// dispatcher running (or a lagging one) the event is simply dropped.
    pub fn emit(&self, event: WebhookEvent) {
        let _ = self
            .tx
            .send(Arc::new(event));
    }

    /// Whether any enabled webhook subscribes to `notification_type`.
    ///
    /// `emit` is cheap, but the *caller* is not: building an event means
    /// cloning usernames and device names, and for `ItemDeleted` re-reading and
    /// boxing a whole [`db::Media`]. On a `PlaybackProgress` stream with no
    /// webhooks configured that cost is paid per progress tick for nothing.
    /// Guard those sites with this.
    ///
    /// Lock-free (two atomic loads) and deliberately conservative: a pending
    /// reload, or a subscription set too wide for the mask, answers `true`. It
    /// is an optimisation, never the authority — the dispatcher re-checks every
    /// event against the real snapshot.
    ///
    /// The `dirty` half is not redundant with the widening in [`Self::invalidate`],
    /// and leaving it out is a *sticky* bug rather than a transient one. The
    /// mask is narrowed by `reload` from a snapshot it read some time earlier,
    /// so an `invalidate` that lands mid-reload has its widen clobbered by a
    /// mask that predates it. Were `wants` to answer from the mask alone, it
    /// would then suppress exactly the guarded events that would otherwise have
    /// woken the dispatcher and made it consume the still-set `dirty` flag — so
    /// nothing would heal it until some *unguarded* event happened to fire,
    /// which on a quiet server can be hours. Consulting `dirty` keeps the
    /// staleness self-healing, which is what it was before this probe existed.
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
        // the dispatcher reloads. `reload` declines to narrow again while the
        // flag is still up, and `wants` consults the flag too, so this is the
        // fast path rather than the correctness argument.
        self.inner
            .wanted_mask
            .store(u32::MAX, Ordering::Relaxed);
        self.inner
            .dirty
            .store(true, Ordering::Release);
    }

    /// Replace the cached snapshot from the database. On error the previous
    /// hook set is kept and the cache is marked stale again — a transient DB
    /// failure must not silently disable every webhook.
    ///
    /// Re-raising `dirty` is what makes that promise true for the *first*
    /// reload, and only the first reload can break it: [`Self::spawn_dispatcher`]
    /// calls this when the previous set is [`LoadedWebhooks::default`], i.e.
    /// empty, so "keep the previous set" keeps nothing. A `SQLITE_BUSY` at boot
    /// would otherwise leave the cache empty with the flag down — nothing to
    /// retry the load, and `wanted_mask` still `u32::MAX` from [`Self::new`], so
    /// every guarded call site keeps paying full price to build events the
    /// dispatcher then discards. Recovery would need an admin to touch a
    /// webhook, or a restart.
    ///
    /// The server identity is reloaded here too, which is why settings writers
    /// call [`Self::invalidate`]: it is built once and then read by every
    /// payload, so a rename would otherwise ship the old name until restart.
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
                // Ask for another attempt. Without this the failure is
                // permanent: the flag was consumed before the call, so nothing
                // else will ever set it.
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
        // Published after the snapshot, and only when the flag is down. On the
        // dispatcher's steady-state path the flag was consumed just before this
        // call, so finding it set again means `hooks` predates an `invalidate`
        // whose widening this store would otherwise silently clobber — leaving
        // the mask narrow, and stale, for as long as the flag stays unconsumed.
        // The startup reload has no preceding swap, so there the check simply
        // holds the mask open until a snapshot nobody has invalidated lands.
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

    /// Whether `hook` wants `event`. Pure: `item_kind` is the kind of the item
    /// the event is about, or `None` when the event carries no item.
    pub(crate) fn matches(
        hook: &db::Webhook,
        event: &WebhookEvent,
        item_kind: Option<&db::MediaKind>,
    ) -> bool {
        // 1. Subscription. An empty list matches nothing — this mirrors the
        //    Jellyfin webhook plugin and is not an oversight.
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
    /// channel drops sends that happen while it has no subscriber, and
    /// `init_app` starts emitting (library scan, startup tasks) before the
    /// spawned task gets its first poll.
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
                // An item-scoped event whose item could not be resolved has
                // nothing left to deliver, and delivering it anyway is worse
                // than dropping it twice over: `item_kind` is `None`, so
                // `matches` skips the item-type rule entirely and a hook with
                // every type unticked fires; and the dictionary has no `Name`,
                // `ItemId` or `ItemType`, so the stock template renders
                // `"title": " () has been added to remux"`. `ItemDeleted`
                // carries its row inline and never lands here.
                if event
                    .item_id()
                    .is_some()
                    && item.is_none()
                {
                    // `debug`, not `warn`: `enrich_item` already logged the
                    // real cause at warn, and a scan that deletes rows behind
                    // an in-flight event makes this expected rather than wrong.
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

                // Built once per event; `render` applies the per-hook overlay
                // (a Generic destination's operator-defined fields).
                let data = payload::build_data(&cache.server, &event, item.as_ref());
                for hook in targets {
                    match template::render(hook, &cache.registry, &data) {
                        // Delivery is spawned so one slow endpoint cannot stall
                        // the dispatcher or the hooks behind it, and bounded so
                        // a dead one cannot grow tasks without limit.
                        Ok(Some(body)) => {
                            sender::spawn_delivery(hook.clone(), body);
                        }
                        // `skip_empty_message_body` suppressed the delivery.
                        Ok(None) => {}
                        // Throttled: the failure is a property of the template,
                        // not of the event, so an unthrottled line repeats for
                        // every tick of a `PlaybackProgress` subscription.
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

/// Whether an operator-supplied template parses, for write-time validation.
///
/// The error is handlebars' own parse error, derived from the operator's text
/// and nothing else — no remote response, no URL — so it is safe to return over
/// the API. Rejecting at write time is the difference between "your template
/// has an unclosed block on line 4" and a hook that saves clean, says
/// "Template not found" when tested, and stays silent in production.
pub fn validate_template(template: &str) -> Result<(), handlebars::TemplateError> {
    template::validate(template)
}

/// Render `hook`'s body for the synthetic test event.
///
/// The template is compiled here rather than taken from the dispatcher's cached
/// registry: the hook being tested was very likely saved a moment ago, and that
/// cache only reloads when the dispatcher next sees an event. Testing a hook
/// against a stale template would be worse than not testing it.
///
/// Compiled through [`template::single_registry`], not `build_registry`: the
/// latter warns-and-skips an unparseable template, which is right for the
/// dispatcher — one hook's typo must not stop the others — and wrong here,
/// because `render` would then fail with handlebars' "Template not found:
/// <uuid>" while the operator's actual syntax error went only to the server
/// log.
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
/// Deliberately not routed through [`WebhookService::emit`]: the broadcast path
/// is fire-and-forget, filtered by the hook's own subscription and retried in
/// the background, and none of that can answer "did *this* webhook work?".
/// A hook that is disabled, or subscribes to nothing, is still testable — that
/// is the point of the button.
///
/// One attempt, no retry, and the answer handed straight back to the caller.
pub async fn deliver_test(ctx: &AppContext, hook: &db::Webhook) -> WebhookTestResult {
    let server = payload::ServerInfo::load(ctx).await;
    match test_body(&server, hook) {
        Ok(Some(body)) => sender::send_test(hook, &body).await,
        // `skip_empty_message_body` would drop this delivery in production, so
        // reporting a success here would be a lie.
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

    /// The dashboard's test button is only useful if the body it sends is the
    /// body a real event would send, built from the same dictionary.
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

    /// The hook's template is compiled for this call, so a hook the dispatcher
    /// has never seen is still testable.
    ///
    /// And what comes back must be the *parse* error. Routed through
    /// `build_registry` this failed with "Template not found: <uuid>", naming
    /// an id the operator never typed while the real error went to the log —
    /// so asserting `is_err()` alone was not enough to keep it honest.
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

    /// The same error is what the CRUD endpoints refuse a write with, so it has
    /// to name something the operator can act on.
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

    /// `skip_empty_message_body` drops the delivery in production; the test
    /// endpoint must say so rather than claim a success it never attempted.
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

    /// The registry is built in two places (here and in `reload`). Both must
    /// carry the custom helpers, or every template using one breaks until — or
    /// from — the first `invalidate()`.
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

    /// The startup reload is the one call with nothing to "keep": the previous
    /// set is `LoadedWebhooks::default()`, i.e. empty. A transient
    /// `SQLITE_BUSY` there used to be permanent — the flag had already been
    /// consumed, so nothing would ever ask for another attempt, and every
    /// webhook stayed disabled until an admin touched one or the process
    /// restarted.
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

    /// The mirror image: a load that worked must not ask to be redone, or the
    /// dispatcher reloads on every single event.
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

    /// Every notification type must own a bit. Two types sharing one would make
    /// `wants` answer for the wrong subscription — and the bit index is the
    /// enum's discriminant, which nothing else in the code pins down.
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

    /// Before the dispatcher's first load — and for the whole window a pending
    /// reload is open — the probe must not tell callers to skip building
    /// events. Skipping is only ever correct against a snapshot that is known
    /// to be current.
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

    /// The narrowing store at the end of `reload` publishes a mask derived from
    /// rows read some time earlier. An `invalidate` that lands in between must
    /// not have its widening clobbered by it.
    ///
    /// This is the interleaving, in order: the dispatcher consumes the flag and
    /// starts reloading, the operator saves a hook mid-reload, the reload
    /// finishes from its now-outdated snapshot. Left unhandled the result is
    /// *sticky*, not transient — the closed probe suppresses exactly the
    /// guarded events that would have woken the dispatcher into consuming the
    /// flag, so nothing reopens it until some unguarded event happens to fire.
    #[tokio::test]
    async fn a_reload_that_races_an_invalidate_leaves_the_probe_open() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .expect("test server");
        let service = WebhookService::new();

        // The dispatcher takes the flag and begins reading the database, which
        // at this point holds no webhooks at all — so this reload can only
        // compute an empty mask.
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

    /// Deliberate parity with the Jellyfin webhook plugin: a webhook that
    /// subscribes to nothing receives nothing, even with every other filter
    /// wide open.
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

    /// Each kind is gated by exactly one flag: enabling only that flag matches,
    /// and disabling only that flag (every other flag on) does not. Together
    /// these pin the mapping — a kind wired to the wrong flag fails both halves.
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
