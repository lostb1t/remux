use anyhow::Result;
use chrono::{DateTime, Utc};
use remux_sdks::remux::{
    NotificationType, WebhookDestination, WebhookDto, WebhookItemTypes,
};
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, types::Json};
use uuid::Uuid;

/// A stored outgoing webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Webhook {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub template: String,
    #[sqlx(json)]
    pub destination: WebhookDestination,
    #[sqlx(json)]
    pub notification_types: Vec<NotificationType>,
    #[sqlx(json)]
    pub user_filter: Vec<Uuid>,
    #[sqlx(json)]
    pub item_types: WebhookItemTypes,
    pub send_all_properties: bool,
    pub trim_whitespace: bool,
    pub skip_empty_message_body: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Webhook {
    /// Insert a new webhook. The id carried by `dto` is ignored — the server
    /// always assigns a fresh one.
    pub async fn create(db: &SqlitePool, dto: &WebhookDto) -> Result<Self> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO webhooks
                (id, name, enabled, url, template, destination, notification_types,
                 user_filter, item_types, send_all_properties, trim_whitespace,
                 skip_empty_message_body, created_at, updated_at)
             VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        )
        .bind(id)
        .bind(&dto.name)
        .bind(dto.enabled)
        .bind(&dto.url)
        .bind(&dto.template)
        .bind(Json(&dto.destination))
        .bind(Json(&dto.notification_types))
        .bind(Json(&dto.user_filter))
        .bind(Json(&dto.item_types))
        .bind(dto.send_all_properties)
        .bind(dto.trim_whitespace)
        .bind(dto.skip_empty_message_body)
        .bind(now)
        .execute(db)
        .await?;

        Self::get_by_id(db, &id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("webhook not found after insert"))
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM webhooks WHERE id = ?1")
                .bind(id)
                .fetch_optional(db)
                .await?,
        )
    }

    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM webhooks ORDER BY created_at")
                .fetch_all(db)
                .await?,
        )
    }

    pub async fn get_enabled(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM webhooks WHERE enabled = 1 ORDER BY created_at",
        )
        .fetch_all(db)
        .await?)
    }

    /// Overwrite every mutable column. `created_at` is preserved and
    /// `updated_at` is bumped to now.
    pub async fn update(db: &SqlitePool, id: &Uuid, dto: &WebhookDto) -> Result<Self> {
        sqlx::query(
            "UPDATE webhooks SET
                name                    = ?2,
                enabled                 = ?3,
                url                     = ?4,
                template                = ?5,
                destination             = ?6,
                notification_types      = ?7,
                user_filter             = ?8,
                item_types              = ?9,
                send_all_properties     = ?10,
                trim_whitespace         = ?11,
                skip_empty_message_body = ?12,
                updated_at              = ?13
             WHERE id = ?1",
        )
        .bind(id)
        .bind(&dto.name)
        .bind(dto.enabled)
        .bind(&dto.url)
        .bind(&dto.template)
        .bind(Json(&dto.destination))
        .bind(Json(&dto.notification_types))
        .bind(Json(&dto.user_filter))
        .bind(Json(&dto.item_types))
        .bind(dto.send_all_properties)
        .bind(dto.trim_whitespace)
        .bind(dto.skip_empty_message_body)
        .bind(Utc::now())
        .execute(db)
        .await?;

        Self::get_by_id(db, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("webhook {id} not found"))
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM webhooks WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub fn into_dto(self) -> WebhookDto {
        WebhookDto {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            url: self.url,
            template: self.template,
            destination: self.destination,
            notification_types: self.notification_types,
            user_filter: self.user_filter,
            item_types: self.item_types,
            send_all_properties: self.send_all_properties,
            trim_whitespace: self.trim_whitespace,
            skip_empty_message_body: self.skip_empty_message_body,
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::remux::{DiscordMentionType, WebhookKeyValue};

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    fn sample_dto() -> WebhookDto {
        WebhookDto {
            id: Uuid::new_v4(),
            name: "discord".into(),
            enabled: true,
            url: "https://example.test/hook".into(),
            template: "{{ItemName}}".into(),
            destination: WebhookDestination::Discord {
                avatar_url: Some("https://example.test/avatar.png".into()),
                bot_username: Some("remux".into()),
                embed_color: Some("#AA5CC3".into()),
                mention_type: DiscordMentionType::Here,
            },
            notification_types: vec![
                NotificationType::ItemAdded,
                NotificationType::PlaybackStart,
            ],
            user_filter: vec![Uuid::new_v4()],
            item_types: WebhookItemTypes {
                songs: false,
                ..Default::default()
            },
            // Distinct values on purpose: these three bools sit on adjacent
            // placeholders of the same type, so identical values would let any
            // permutation of the binds pass. This shape alone still cannot
            // catch a 1↔3 swap (both are `true`) — see `FLAG_CASES` below.
            send_all_properties: true,
            trim_whitespace: false,
            skip_empty_message_body: true,
            created_at: None,
            updated_at: None,
        }
    }

    /// `(send_all_properties, trim_whitespace, skip_empty_message_body)`.
    ///
    /// Three booleans only take two values, so no single combination can
    /// distinguish all three pairwise swaps: whichever shape is chosen, one
    /// pair holds the same value and swapping it is invisible. These one-hot
    /// cases cover the three swaps between them — each case detects the two
    /// swaps that involve its single `true`:
    ///
    /// | case          | 1↔2 | 2↔3 | 1↔3 |
    /// |---------------|-----|-----|-----|
    /// | `(T, F, F)`   | ✓   |     | ✓   |
    /// | `(F, T, F)`   | ✓   | ✓   |     |
    /// | `(F, F, T)`   |     | ✓   | ✓   |
    const FLAG_CASES: [(bool, bool, bool); 3] = [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ];

    fn assert_flags(webhook: &Webhook, expected: (bool, bool, bool), stage: &str) {
        assert_eq!(
            (
                webhook.send_all_properties,
                webhook.trim_whitespace,
                webhook.skip_empty_message_body,
            ),
            expected,
            "{stage}: (send_all_properties, trim_whitespace, skip_empty_message_body)"
        );
    }

    /// Guards against a transposed `.bind()` among the three adjacent boolean
    /// placeholders in `create` and in `update`.
    #[tokio::test]
    async fn boolean_flags_land_in_their_own_columns() {
        let db = test_db().await;

        for (send_all, trim, skip_empty) in FLAG_CASES {
            let expected = (send_all, trim, skip_empty);
            let dto = WebhookDto {
                send_all_properties: send_all,
                trim_whitespace: trim,
                skip_empty_message_body: skip_empty,
                ..sample_dto()
            };

            // create writes the triple.
            let created = Webhook::create(&db, &dto)
                .await
                .unwrap();
            assert_flags(&created, expected, "create returned");
            assert_flags(
                &Webhook::get_by_id(&db, &created.id)
                    .await
                    .unwrap()
                    .expect("created webhook must be readable back"),
                expected,
                "create stored",
            );

            // update writes the triple onto a row currently holding its inverse,
            // so every column has to be written to reach the expected state.
            let seed = Webhook::create(
                &db,
                &WebhookDto {
                    send_all_properties: !send_all,
                    trim_whitespace: !trim,
                    skip_empty_message_body: !skip_empty,
                    ..sample_dto()
                },
            )
            .await
            .unwrap();
            let updated = Webhook::update(&db, &seed.id, &dto)
                .await
                .unwrap();
            assert_flags(&updated, expected, "update returned");
            assert_flags(
                &Webhook::get_by_id(&db, &seed.id)
                    .await
                    .unwrap()
                    .expect("updated webhook must be readable back"),
                expected,
                "update stored",
            );
        }
    }

    #[tokio::test]
    async fn create_assigns_a_fresh_id_and_persists_scalar_columns() {
        let db = test_db().await;
        let dto = sample_dto();

        let created = Webhook::create(&db, &dto)
            .await
            .unwrap();

        assert_ne!(created.id, dto.id, "create must ignore the incoming id");
        assert_eq!(created.name, dto.name);
        assert_eq!(created.enabled, dto.enabled);
        assert_eq!(created.url, dto.url);
        assert_eq!(created.template, dto.template);
        assert_eq!(created.send_all_properties, dto.send_all_properties);
        assert_eq!(created.trim_whitespace, dto.trim_whitespace);
        assert_eq!(created.skip_empty_message_body, dto.skip_empty_message_body);

        let fetched = Webhook::get_by_id(&db, &created.id)
            .await
            .unwrap()
            .expect("webhook must be readable back");
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "discord");
        assert_eq!(fetched.url, "https://example.test/hook");
        assert_eq!(fetched.template, "{{ItemName}}");
        assert!(fetched.enabled);
        assert!(fetched.send_all_properties);
        assert!(!fetched.trim_whitespace);
        assert!(fetched.skip_empty_message_body);
    }

    #[tokio::test]
    async fn get_by_id_returns_none_for_unknown_id() {
        let db = test_db().await;
        assert!(
            Webhook::get_by_id(&db, &Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn json_columns_round_trip() {
        let db = test_db().await;
        let users = vec![Uuid::new_v4(), Uuid::new_v4()];
        let dto = WebhookDto {
            destination: WebhookDestination::Generic {
                headers: vec![WebhookKeyValue {
                    key: "X-Token".into(),
                    value: "secret".into(),
                }],
                fields: vec![
                    WebhookKeyValue {
                        key: "channel".into(),
                        value: "#general".into(),
                    },
                    WebhookKeyValue {
                        key: "kind".into(),
                        value: "alert".into(),
                    },
                ],
            },
            notification_types: vec![
                NotificationType::ItemAdded,
                NotificationType::UserPasswordChanged,
            ],
            user_filter: users.clone(),
            item_types: WebhookItemTypes {
                movies: true,
                episodes: false,
                series: true,
                seasons: false,
                albums: true,
                songs: false,
                videos: true,
            },
            ..sample_dto()
        };

        let created = Webhook::create(&db, &dto)
            .await
            .unwrap();
        let fetched = Webhook::get_by_id(&db, &created.id)
            .await
            .unwrap()
            .expect("webhook must be readable back");

        // The tagged enum must come back as the same variant, with payload intact.
        match &fetched.destination {
            WebhookDestination::Generic { headers, fields } => {
                assert_eq!(headers.len(), 1);
                assert_eq!(headers[0].key, "X-Token");
                assert_eq!(headers[0].value, "secret");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[1].key, "kind");
            }
            other => panic!("expected Generic destination, got {other:?}"),
        }
        assert_eq!(fetched.destination, dto.destination);
        assert_eq!(fetched.notification_types, dto.notification_types);
        assert_eq!(fetched.user_filter, users);
        assert_eq!(fetched.item_types, dto.item_types);
        assert!(
            !fetched
                .item_types
                .episodes
        );
        assert!(
            !fetched
                .item_types
                .songs
        );
    }

    #[tokio::test]
    async fn discord_destination_round_trips() {
        let db = test_db().await;
        let created = Webhook::create(&db, &sample_dto())
            .await
            .unwrap();
        let fetched = Webhook::get_by_id(&db, &created.id)
            .await
            .unwrap()
            .expect("webhook must be readable back");

        match fetched.destination {
            WebhookDestination::Discord {
                avatar_url,
                bot_username,
                embed_color,
                mention_type,
            } => {
                assert_eq!(
                    avatar_url.as_deref(),
                    Some("https://example.test/avatar.png")
                );
                assert_eq!(bot_username.as_deref(), Some("remux"));
                assert_eq!(embed_color.as_deref(), Some("#AA5CC3"));
                assert_eq!(mention_type, DiscordMentionType::Here);
            }
            other => panic!("expected Discord destination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_all_is_ordered_by_created_at() {
        let db = test_db().await;
        for name in ["first", "second", "third"] {
            Webhook::create(
                &db,
                &WebhookDto {
                    name: name.into(),
                    ..sample_dto()
                },
            )
            .await
            .unwrap();
        }

        let all = Webhook::get_all(&db)
            .await
            .unwrap();
        assert_eq!(
            all.iter()
                .map(|w| w
                    .name
                    .as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
    }

    #[tokio::test]
    async fn get_enabled_excludes_disabled_rows() {
        let db = test_db().await;
        let on = Webhook::create(
            &db,
            &WebhookDto {
                name: "on".into(),
                enabled: true,
                ..sample_dto()
            },
        )
        .await
        .unwrap();
        let off = Webhook::create(
            &db,
            &WebhookDto {
                name: "off".into(),
                enabled: false,
                ..sample_dto()
            },
        )
        .await
        .unwrap();

        let enabled = Webhook::get_enabled(&db)
            .await
            .unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, on.id);

        // The disabled row still exists — get_enabled filters, it does not delete.
        assert_eq!(
            Webhook::get_all(&db)
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(
            !Webhook::get_by_id(&db, &off.id)
                .await
                .unwrap()
                .expect("disabled webhook still stored")
                .enabled
        );
    }

    #[tokio::test]
    async fn update_replaces_json_columns_and_advances_updated_at() {
        let db = test_db().await;
        let created = Webhook::create(&db, &sample_dto())
            .await
            .unwrap();

        let patch = WebhookDto {
            id: Uuid::new_v4(),
            name: "renamed".into(),
            enabled: false,
            url: "https://example.test/other".into(),
            template: "{{SeriesName}}".into(),
            destination: WebhookDestination::Generic {
                headers: vec![WebhookKeyValue {
                    key: "Authorization".into(),
                    value: "Bearer x".into(),
                }],
                fields: vec![],
            },
            notification_types: vec![NotificationType::PlaybackStop],
            user_filter: vec![],
            item_types: WebhookItemTypes {
                movies: false,
                ..Default::default()
            },
            // Inverse of the fixture, and still distinct from each other, so a
            // transposed bind in `update` cannot go unnoticed either.
            send_all_properties: false,
            trim_whitespace: true,
            skip_empty_message_body: false,
            created_at: None,
            updated_at: None,
        };

        let updated = Webhook::update(&db, &created.id, &patch)
            .await
            .unwrap();

        assert_eq!(updated.id, created.id, "update must not re-key the row");
        assert_eq!(updated.name, "renamed");
        assert!(!updated.enabled);
        assert_eq!(updated.url, "https://example.test/other");
        assert_eq!(updated.template, "{{SeriesName}}");
        assert!(!updated.send_all_properties);
        assert!(updated.trim_whitespace);
        assert!(!updated.skip_empty_message_body);

        // JSON columns actually changed.
        assert_ne!(updated.destination, created.destination);
        assert_eq!(updated.destination, patch.destination);
        assert_eq!(
            updated.notification_types,
            vec![NotificationType::PlaybackStop]
        );
        assert!(
            updated
                .user_filter
                .is_empty()
        );
        assert!(
            !updated
                .item_types
                .movies
        );

        assert_eq!(
            updated.created_at, created.created_at,
            "created_at must be preserved"
        );
        assert!(
            updated.updated_at > created.updated_at,
            "updated_at must advance ({} !> {})",
            updated.updated_at,
            created.updated_at
        );

        // The change is persisted, not just reflected in the returned value.
        let fetched = Webhook::get_by_id(&db, &created.id)
            .await
            .unwrap()
            .expect("webhook must still exist");
        assert_eq!(fetched.destination, patch.destination);
        assert_eq!(fetched.name, "renamed");
        assert_eq!(fetched.updated_at, updated.updated_at);
        assert!(!fetched.send_all_properties);
        assert!(fetched.trim_whitespace);
        assert!(!fetched.skip_empty_message_body);
    }

    #[tokio::test]
    async fn delete_removes_only_the_target_row() {
        let db = test_db().await;
        let a = Webhook::create(
            &db,
            &WebhookDto {
                name: "a".into(),
                ..sample_dto()
            },
        )
        .await
        .unwrap();
        let b = Webhook::create(
            &db,
            &WebhookDto {
                name: "b".into(),
                ..sample_dto()
            },
        )
        .await
        .unwrap();

        Webhook::delete(&db, &a.id)
            .await
            .unwrap();

        assert!(
            Webhook::get_by_id(&db, &a.id)
                .await
                .unwrap()
                .is_none()
        );
        let all = Webhook::get_all(&db)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, b.id);
    }

    #[tokio::test]
    async fn into_dto_exposes_stored_timestamps() {
        let db = test_db().await;
        let created = Webhook::create(&db, &sample_dto())
            .await
            .unwrap();
        let (id, created_at, updated_at, destination) = (
            created.id,
            created.created_at,
            created.updated_at,
            created
                .destination
                .clone(),
        );

        let dto = created.into_dto();

        assert_eq!(dto.id, id);
        assert_eq!(dto.created_at, Some(created_at));
        assert_eq!(dto.updated_at, Some(updated_at));
        assert_eq!(dto.destination, destination);
        assert_eq!(dto.name, "discord");
        assert!(dto.enabled);
    }
}
