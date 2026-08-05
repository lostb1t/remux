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

pub use events::{
    DeviceEventData, PlaybackEventData, UserDataSaveReason, UserEventData, WebhookEvent,
};

use crate::{AppContext, db};
use remux_sdks::remux::NotificationType;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{RwLock, broadcast, broadcast::error::RecvError},
    task::JoinHandle,
};
use tracing::warn;

/// Buffered events per subscriber. Large enough that a slow dispatcher pass
/// (one enrichment round-trip) never drops events under normal playback load.
const EVENT_CHANNEL_CAPACITY: usize = 4096;

/// The enabled webhooks as last read from the database, plus everything derived
/// from them that would otherwise be recomputed per event.
pub(crate) struct LoadedWebhooks {
    pub hooks: Vec<db::Webhook>,
    /// Every hook's template, pre-compiled under its id, plus the custom helpers.
    pub registry: handlebars::Handlebars<'static>,
    /// Union of every enabled hook's subscriptions — the dispatcher fast-path.
    pub wanted: HashSet<NotificationType>,
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
        }
    }
}

struct Inner {
    /// Set by the webhook CRUD endpoints; consumed by the dispatcher.
    dirty: AtomicBool,
    cache: RwLock<LoadedWebhooks>,
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

    /// Mark the cached webhook set stale. The dispatcher reloads before it
    /// handles the next event.
    pub fn invalidate(&self) {
        self.inner
            .dirty
            .store(true, Ordering::Release);
    }

    /// Replace the cached snapshot from the database. On error the previous
    /// snapshot is kept — a transient DB failure must not silently disable
    /// every webhook.
    ///
    /// Invariant: this is the only writer of `cache`, and it is only ever
    /// called from the dispatcher task itself, at a point where that task
    /// holds no read guard. That is what makes it safe for the dispatcher to
    /// hold the read guard across `enrich_item().await` — no other task can be
    /// waiting for the write lock.
    async fn reload(&self, db: &sqlx::SqlitePool) {
        let hooks = match db::Webhook::get_enabled(db).await {
            Ok(hooks) => hooks,
            Err(e) => {
                warn!(error = %e, "failed to load webhooks, keeping previous set");
                return;
            }
        };
        let wanted = hooks
            .iter()
            .flat_map(|hook| {
                hook.notification_types
                    .iter()
                    .copied()
            })
            .collect();
        let mut cache = self
            .inner
            .cache
            .write()
            .await;
        *cache = LoadedWebhooks {
            registry: template::build_registry(&hooks),
            hooks,
            wanted,
        };
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
            self.reload(&ctx.db)
                .await;
            // Read once: every event of this process reports the same server.
            let server = payload::ServerInfo::load(&ctx).await;

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
                    self.reload(&ctx.db)
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
                let data = payload::build_data(&server, &event, item.as_ref());
                for hook in targets {
                    match template::render(hook, &cache.registry, &data) {
                        // Delivery is spawned so one slow endpoint cannot stall
                        // the dispatcher or the hooks behind it.
                        Ok(Some(body)) => {
                            tokio::spawn(sender::deliver(hook.clone(), body));
                        }
                        // `skip_empty_message_body` suppressed the delivery.
                        Ok(None) => {}
                        Err(e) => {
                            warn!(webhook = %hook.name, error = %e, "webhook template render failed")
                        }
                    }
                }
            }
        })
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
