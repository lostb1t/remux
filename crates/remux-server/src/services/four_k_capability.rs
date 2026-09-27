use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use remux_sdks::remux::{MediaSourceInfo, MediaStreamType, PlayMethod};
use uuid::Uuid;

use crate::{
    AppContext,
    db::auth::Device,
    signals::{Event, EventType, PlaybackPosition, Subscriber},
};

const REQUIRED_PLAYBACK: Duration = Duration::from_secs(10 * 60);
const MAX_REPORT_GAP: Duration = Duration::from_secs(3 * 60);
const OBSERVATION_TTL: Duration = Duration::from_secs(60 * 60);

type SessionKey = (Uuid, String, String);
type DeviceKey = (Uuid, String);

struct Observation {
    source_id: String,
    last_at: Instant,
    last_position_ticks: i64,
    played: Duration,
}

#[derive(Default)]
struct TrackingState {
    sessions: HashMap<SessionKey, Observation>,
    confirmed: HashSet<DeviceKey>,
}

pub struct FourKCapabilitySubscriber {
    pub ctx: AppContext,
    state: Mutex<TrackingState>,
}

impl FourKCapabilitySubscriber {
    pub fn new(ctx: AppContext) -> Self {
        Self {
            ctx,
            state: Mutex::new(TrackingState::default()),
        }
    }

    async fn observe(&self, report: PlaybackPosition) -> anyhow::Result<()> {
        let info = report.context;
        let device_key = (
            info.user_id,
            info.device_id
                .clone(),
        );
        let Some(source_id) = info.media_source_id else {
            return Ok(());
        };
        let Ok(source_id) = Uuid::parse_str(&source_id) else {
            return Ok(());
        };
        let source_id = source_id.to_string();
        if info
            .session_id
            .is_empty()
        {
            return Ok(());
        }
        let session_key = (
            info.user_id,
            info.device_id
                .clone(),
            info.session_id
                .clone(),
        );

        if !report.video_is_copied
            || !matches!(
                info.play_method
                    .as_ref(),
                Some(PlayMethod::DirectPlay | PlayMethod::DirectStream)
            )
            || self
                .ctx
                .store
                .get::<()>(source_key(
                    &info.user_id,
                    &info.device_id,
                    &info.session_id,
                    &source_id,
                ))
                .is_none()
        {
            self.state
                .lock()
                .unwrap()
                .sessions
                .remove(&session_key);
            return Ok(());
        }

        let qualified = {
            let mut state = self
                .state
                .lock()
                .unwrap();
            if state
                .confirmed
                .contains(&device_key)
            {
                return Ok(());
            }
            state
                .sessions
                .retain(|_, value| {
                    report
                        .reported_at
                        .saturating_duration_since(value.last_at)
                        < OBSERVATION_TTL
                });
            let observation = state
                .sessions
                .entry(session_key)
                .or_insert_with(|| Observation {
                    source_id: source_id.clone(),
                    last_at: report.reported_at,
                    last_position_ticks: info.position_ticks,
                    played: Duration::ZERO,
                });

            if observation.source_id != source_id {
                *observation = Observation {
                    source_id,
                    last_at: report.reported_at,
                    last_position_ticks: info.position_ticks,
                    played: Duration::ZERO,
                };
                return Ok(());
            }
            let Some(elapsed) = report
                .reported_at
                .checked_duration_since(observation.last_at)
            else {
                return Ok(());
            };
            if elapsed.is_zero() {
                return Ok(());
            }
            let advanced_ticks = info
                .position_ticks
                .checked_sub(observation.last_position_ticks)
                .unwrap_or_default();
            observation.last_at = report.reported_at;
            observation.last_position_ticks = info.position_ticks;
            if info.is_paused || elapsed > MAX_REPORT_GAP || advanced_ticks <= 0 {
                return Ok(());
            }
            let advanced =
                Duration::from_secs_f64(advanced_ticks as f64 / 10_000_000.0);
            // A large jump is a seek, not evidence of time spent decoding.
            if advanced > elapsed.mul_f64(1.5) + Duration::from_secs(5) {
                return Ok(());
            }
            observation.played += advanced.min(elapsed);
            observation.played >= REQUIRED_PLAYBACK
        };

        if qualified {
            Device::mark_4k_capable(
                &self
                    .ctx
                    .db,
                info.user_id,
                &info.device_id,
            )
            .await?;
            let mut state = self
                .state
                .lock()
                .unwrap();
            state
                .confirmed
                .insert(device_key.clone());
            state
                .sessions
                .retain(|key, _| {
                    (
                        key.0,
                        key.1
                            .clone(),
                    ) != device_key
                });
        }
        Ok(())
    }
}

#[async_trait]
impl Subscriber for FourKCapabilitySubscriber {
    fn key(&self) -> &'static str {
        "four_k_capability"
    }

    fn events(&self) -> &[EventType] {
        &[EventType::PlaybackPosition]
    }

    async fn handle(&self, event: Event) -> anyhow::Result<()> {
        match event {
            Event::PlaybackPosition(report) => {
                self.observe(report)
                    .await
            }
            _ => Ok(()),
        }
    }
}

/// Remember the returned source IDs, including PlaybackInfo's item-ID alias.
/// The playback report identifies one of these IDs; an item containing a 4K
/// version is not sufficient evidence unless that version was actually used.
pub fn remember_sources(
    ctx: &AppContext,
    user_id: Uuid,
    device_id: &str,
    play_session_id: &str,
    sources: &[MediaSourceInfo],
    original_first_source_id: Option<Uuid>,
) {
    for (index, source) in sources
        .iter()
        .enumerate()
    {
        let is_4k = source
            .media_streams
            .iter()
            .find(|stream| stream.type_ == Some(MediaStreamType::Video))
            .is_some_and(|stream| {
                let (Some(width), Some(height)) = (stream.width, stream.height) else {
                    return false;
                };
                width.max(height) >= 3200 && width.min(height) >= 1500
            });
        if is_4k {
            ctx.store
                .save(
                    source_key(
                        &user_id,
                        device_id,
                        play_session_id,
                        &source
                            .id
                            .to_string(),
                    ),
                    (),
                    Duration::from_secs(24 * 60 * 60),
                );
            if index == 0
                && let Some(original_id) = original_first_source_id
                && original_id != source.id
            {
                ctx.store
                    .save(
                        source_key(
                            &user_id,
                            device_id,
                            play_session_id,
                            &original_id.to_string(),
                        ),
                        (),
                        Duration::from_secs(24 * 60 * 60),
                    );
            }
        }
    }
}

fn source_key(
    user_id: &Uuid,
    device_id: &str,
    session_id: &str,
    source_id: &str,
) -> String {
    format!("observed-4k-source:{user_id}:{device_id}:{session_id}:{source_id}")
}
