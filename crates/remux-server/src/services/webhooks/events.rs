//! The internal webhook event type.
//!
//! Events carry the data that is already in hand at the emission point so that
//! emitting never touches the database. Anything else the templates need is
//! resolved later, off the hot path, by the dispatcher.

use crate::db;
use remux_sdks::remux::NotificationType;
use uuid::Uuid;

/// The user a webhook event is attributed to.
#[derive(Debug, Clone)]
pub struct UserEventData {
    pub id: Uuid,
    pub username: String,
}

/// The client/device a webhook event originated from.
#[derive(Debug, Clone)]
pub struct DeviceEventData {
    pub id: String,
    pub name: String,
    pub client_name: String,
    pub remote_ip: Option<String>,
}

/// The playback state shared by the three playback events.
#[derive(Debug, Clone)]
pub struct PlaybackEventData {
    pub user: UserEventData,
    pub item_id: Uuid,
    pub device: DeviceEventData,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub play_method: Option<String>,
}

/// Why a `UserDataSaved` event was raised.
#[derive(Debug, Clone, Copy, strum_macros::Display)]
pub enum UserDataSaveReason {
    TogglePlayed,
    ToggleFavorite,
    PlaybackProgress,
    PlaybackFinished,
}

/// One server-side occurrence a webhook can subscribe to.
///
/// Maps 1:1 onto [`NotificationType`] — see [`WebhookEvent::notification_type`].
#[derive(Debug, Clone)]
pub enum WebhookEvent {
    ItemAdded {
        item_id: Uuid,
    },
    /// The row is captured *before* the DELETE, so the payload can still be built.
    ItemDeleted {
        item: Box<db::Media>,
    },
    Generic {
        title: String,
        extra: Vec<(String, String)>,
    },
    PlaybackStart {
        playback: PlaybackEventData,
    },
    PlaybackProgress {
        playback: PlaybackEventData,
    },
    PlaybackStop {
        playback: PlaybackEventData,
    },
    AuthenticationSuccess {
        user: UserEventData,
        device: DeviceEventData,
    },
    AuthenticationFailure {
        username: String,
        remote_ip: Option<String>,
    },
    SessionStart {
        user: UserEventData,
        device: DeviceEventData,
    },
    TaskCompleted {
        key: String,
        name: String,
        succeeded: bool,
        elapsed_ms: u64,
    },
    UserCreated {
        user: UserEventData,
    },
    UserDeleted {
        user_id: Uuid,
        username: String,
    },
    UserUpdated {
        user: UserEventData,
    },
    UserPasswordChanged {
        user: UserEventData,
    },
    UserDataSaved {
        user: UserEventData,
        item_id: Uuid,
        save_reason: UserDataSaveReason,
    },
}

impl WebhookEvent {
    /// The subscription key operators pick in the admin UI.
    pub fn notification_type(&self) -> NotificationType {
        match self {
            Self::ItemAdded { .. } => NotificationType::ItemAdded,
            Self::ItemDeleted { .. } => NotificationType::ItemDeleted,
            Self::Generic { .. } => NotificationType::Generic,
            Self::PlaybackStart { .. } => NotificationType::PlaybackStart,
            Self::PlaybackProgress { .. } => NotificationType::PlaybackProgress,
            Self::PlaybackStop { .. } => NotificationType::PlaybackStop,
            Self::AuthenticationSuccess { .. } => {
                NotificationType::AuthenticationSuccess
            }
            Self::AuthenticationFailure { .. } => {
                NotificationType::AuthenticationFailure
            }
            Self::SessionStart { .. } => NotificationType::SessionStart,
            Self::TaskCompleted { .. } => NotificationType::TaskCompleted,
            Self::UserCreated { .. } => NotificationType::UserCreated,
            Self::UserDeleted { .. } => NotificationType::UserDeleted,
            Self::UserUpdated { .. } => NotificationType::UserUpdated,
            Self::UserPasswordChanged { .. } => NotificationType::UserPasswordChanged,
            Self::UserDataSaved { .. } => NotificationType::UserDataSaved,
        }
    }

    /// The user this event is attributed to, if any. `None` disables the
    /// per-webhook user filter for this event.
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::PlaybackStart { playback }
            | Self::PlaybackProgress { playback }
            | Self::PlaybackStop { playback } => Some(
                playback
                    .user
                    .id,
            ),
            Self::AuthenticationSuccess { user, .. }
            | Self::SessionStart { user, .. }
            | Self::UserCreated { user }
            | Self::UserUpdated { user }
            | Self::UserPasswordChanged { user }
            | Self::UserDataSaved { user, .. } => Some(user.id),
            Self::UserDeleted { user_id, .. } => Some(*user_id),
            Self::ItemAdded { .. }
            | Self::ItemDeleted { .. }
            | Self::Generic { .. }
            | Self::AuthenticationFailure { .. }
            | Self::TaskCompleted { .. } => None,
        }
    }

    /// The library item this event is about, if any.
    pub fn item_id(&self) -> Option<Uuid> {
        match self {
            Self::ItemAdded { item_id } | Self::UserDataSaved { item_id, .. } => {
                Some(*item_id)
            }
            Self::ItemDeleted { item } => Some(item.id),
            Self::PlaybackStart { playback }
            | Self::PlaybackProgress { playback }
            | Self::PlaybackStop { playback } => Some(playback.item_id),
            Self::Generic { .. }
            | Self::AuthenticationSuccess { .. }
            | Self::AuthenticationFailure { .. }
            | Self::SessionStart { .. }
            | Self::TaskCompleted { .. }
            | Self::UserCreated { .. }
            | Self::UserDeleted { .. }
            | Self::UserUpdated { .. }
            | Self::UserPasswordChanged { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn user() -> UserEventData {
        UserEventData {
            id: Uuid::from_u128(1),
            username: "alice".into(),
        }
    }

    fn device() -> DeviceEventData {
        DeviceEventData {
            id: "device-1".into(),
            name: "Living Room".into(),
            client_name: "Jellyfin Web".into(),
            remote_ip: Some("10.0.0.2".into()),
        }
    }

    fn playback() -> PlaybackEventData {
        PlaybackEventData {
            user: user(),
            item_id: Uuid::from_u128(2),
            device: device(),
            position_ticks: 123,
            is_paused: false,
            play_method: Some("DirectStream".into()),
        }
    }

    fn media() -> Box<db::Media> {
        Box::new(db::Media {
            id: Uuid::from_u128(3),
            ..Default::default()
        })
    }

    /// Compile-time guard. Adding a variant to [`WebhookEvent`] breaks this
    /// match, which forces `EVENTS` (and therefore the mapping under test) to
    /// be extended too.
    fn variant_index(event: &WebhookEvent) -> usize {
        match event {
            WebhookEvent::ItemAdded { .. } => 0,
            WebhookEvent::ItemDeleted { .. } => 1,
            WebhookEvent::Generic { .. } => 2,
            WebhookEvent::PlaybackStart { .. } => 3,
            WebhookEvent::PlaybackProgress { .. } => 4,
            WebhookEvent::PlaybackStop { .. } => 5,
            WebhookEvent::AuthenticationSuccess { .. } => 6,
            WebhookEvent::AuthenticationFailure { .. } => 7,
            WebhookEvent::SessionStart { .. } => 8,
            WebhookEvent::TaskCompleted { .. } => 9,
            WebhookEvent::UserCreated { .. } => 10,
            WebhookEvent::UserDeleted { .. } => 11,
            WebhookEvent::UserUpdated { .. } => 12,
            WebhookEvent::UserPasswordChanged { .. } => 13,
            WebhookEvent::UserDataSaved { .. } => 14,
        }
    }

    const VARIANT_COUNT: usize = 15;

    /// One sample per variant, paired with the notification type it must map to.
    fn events() -> Vec<(WebhookEvent, NotificationType)> {
        vec![
            (
                WebhookEvent::ItemAdded {
                    item_id: Uuid::from_u128(2),
                },
                NotificationType::ItemAdded,
            ),
            (
                WebhookEvent::ItemDeleted { item: media() },
                NotificationType::ItemDeleted,
            ),
            (
                WebhookEvent::Generic {
                    title: "hello".into(),
                    extra: vec![],
                },
                NotificationType::Generic,
            ),
            (
                WebhookEvent::PlaybackStart {
                    playback: playback(),
                },
                NotificationType::PlaybackStart,
            ),
            (
                WebhookEvent::PlaybackProgress {
                    playback: playback(),
                },
                NotificationType::PlaybackProgress,
            ),
            (
                WebhookEvent::PlaybackStop {
                    playback: playback(),
                },
                NotificationType::PlaybackStop,
            ),
            (
                WebhookEvent::AuthenticationSuccess {
                    user: user(),
                    device: device(),
                },
                NotificationType::AuthenticationSuccess,
            ),
            (
                WebhookEvent::AuthenticationFailure {
                    username: "mallory".into(),
                    remote_ip: None,
                },
                NotificationType::AuthenticationFailure,
            ),
            (
                WebhookEvent::SessionStart {
                    user: user(),
                    device: device(),
                },
                NotificationType::SessionStart,
            ),
            (
                WebhookEvent::TaskCompleted {
                    key: "scan".into(),
                    name: "Scan library".into(),
                    succeeded: true,
                    elapsed_ms: 42,
                },
                NotificationType::TaskCompleted,
            ),
            (
                WebhookEvent::UserCreated { user: user() },
                NotificationType::UserCreated,
            ),
            (
                WebhookEvent::UserDeleted {
                    user_id: Uuid::from_u128(1),
                    username: "alice".into(),
                },
                NotificationType::UserDeleted,
            ),
            (
                WebhookEvent::UserUpdated { user: user() },
                NotificationType::UserUpdated,
            ),
            (
                WebhookEvent::UserPasswordChanged { user: user() },
                NotificationType::UserPasswordChanged,
            ),
            (
                WebhookEvent::UserDataSaved {
                    user: user(),
                    item_id: Uuid::from_u128(2),
                    save_reason: UserDataSaveReason::TogglePlayed,
                },
                NotificationType::UserDataSaved,
            ),
        ]
    }

    #[test]
    fn every_variant_has_a_sample() {
        let covered: HashSet<usize> = events()
            .iter()
            .map(|(event, _)| variant_index(event))
            .collect();
        assert_eq!(
            covered.len(),
            VARIANT_COUNT,
            "events() must contain exactly one sample per WebhookEvent variant"
        );
    }

    #[test]
    fn notification_type_maps_each_variant() {
        for (event, expected) in events() {
            assert_eq!(
                event.notification_type(),
                expected,
                "wrong notification type for {event:?}"
            );
        }
    }

    /// A mapping that collapses two variants onto the same notification type
    /// would still pass a naive per-case check if the expectations were copied
    /// from the (wrong) implementation. Distinctness is checked separately.
    #[test]
    fn notification_types_are_distinct_across_variants() {
        let produced: HashSet<NotificationType> = events()
            .iter()
            .map(|(event, _)| event.notification_type())
            .collect();
        assert_eq!(
            produced.len(),
            VARIANT_COUNT,
            "each WebhookEvent variant must map to its own NotificationType"
        );
    }

    #[test]
    fn user_id_is_present_only_for_user_attributed_events() {
        let alice = Uuid::from_u128(1);
        let expected: Vec<Option<Uuid>> = vec![
            None,        // ItemAdded
            None,        // ItemDeleted
            None,        // Generic
            Some(alice), // PlaybackStart
            Some(alice), // PlaybackProgress
            Some(alice), // PlaybackStop
            Some(alice), // AuthenticationSuccess
            None,        // AuthenticationFailure
            Some(alice), // SessionStart
            None,        // TaskCompleted
            Some(alice), // UserCreated
            Some(alice), // UserDeleted
            Some(alice), // UserUpdated
            Some(alice), // UserPasswordChanged
            Some(alice), // UserDataSaved
        ];
        assert_eq!(expected.len(), VARIANT_COUNT);
        for ((event, _), want) in events()
            .iter()
            .zip(expected)
        {
            assert_eq!(event.user_id(), want, "wrong user_id for {event:?}");
        }
    }

    #[test]
    fn item_id_is_present_only_for_item_scoped_events() {
        let item = Uuid::from_u128(2);
        let deleted = Uuid::from_u128(3);
        let expected: Vec<Option<Uuid>> = vec![
            Some(item),    // ItemAdded
            Some(deleted), // ItemDeleted — read off the captured row
            None,          // Generic
            Some(item),    // PlaybackStart
            Some(item),    // PlaybackProgress
            Some(item),    // PlaybackStop
            None,          // AuthenticationSuccess
            None,          // AuthenticationFailure
            None,          // SessionStart
            None,          // TaskCompleted
            None,          // UserCreated
            None,          // UserDeleted
            None,          // UserUpdated
            None,          // UserPasswordChanged
            Some(item),    // UserDataSaved
        ];
        assert_eq!(expected.len(), VARIANT_COUNT);
        for ((event, _), want) in events()
            .iter()
            .zip(expected)
        {
            assert_eq!(event.item_id(), want, "wrong item_id for {event:?}");
        }
    }
}
