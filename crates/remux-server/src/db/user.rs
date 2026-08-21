use super::{FilterResult, QueryBuilderExt, Settings};
use crate::{
    IntoApiError, OptionExt, ResultExt,
    api::{ScrollDirection, SortOrder},
    common::get_uuid,
    sdks,
};
use anyhow::{Context, Result, anyhow};
use argon2::{
    Argon2,
    password_hash::{
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
    },
};
use async_trait::async_trait;
use axum::{
    Json, Router, ServiceExt,
    body::Body,
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    middleware,
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_anyhow::{ApiError, ApiResult, on_error, set_expose_errors};
use chrono::{Duration, Utc, prelude::*};
use config::{self, Config};
use default2;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use reqwest::{self, header::LOCATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::{self, collections::HashMap, env, fs, path::Path, sync::Arc};
use timed;
use tower::{Layer, util::MapRequestLayer};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};
use tracing::{self, debug, instrument, warn};
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: remux_utils::Secret<String>,
    #[serde(skip_serializing)]
    pub aio_url: Option<remux_utils::Secret<String>>,
    pub configuration: Option<sqlx::types::Json<crate::api::UserConfiguration>>,
    pub is_admin: bool,
    pub policy: Option<sqlx::types::Json<crate::api::UserPolicy>>,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserFilter {
    pub id: Option<Vec<Uuid>>,
    pub username: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub total_count: bool,
}

impl User {
    pub async fn save(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, username, password_hash, aio_url, configuration, is_admin, policy)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                username      = excluded.username,
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url,
                configuration = excluded.configuration,
                is_admin      = excluded.is_admin,
                policy        = excluded.policy
            "#,
        )
        .bind(self.id)
        .bind(&self.username)
        .bind(&self.password_hash)
        .bind(&self.aio_url)
        .bind(&self.configuration)
        .bind(self.is_admin)
        .bind(&self.policy)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn save_by_username(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, username, password_hash, aio_url, configuration, is_admin, policy)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(username) DO UPDATE SET
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url,
                is_admin      = excluded.is_admin
            "#,
        )
        .bind(self.id)
        .bind(&self.username)
        .bind(&self.password_hash)
        .bind(&self.aio_url)
        .bind(&self.configuration)
        .bind(self.is_admin)
        .bind(&self.policy)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn save_configuration(
        db: &SqlitePool,
        id: &Uuid,
        config: &crate::api::UserConfiguration,
    ) -> Result<()> {
        let json = sqlx::types::Json(config.clone());
        sqlx::query(r#"UPDATE users SET configuration = ?1 WHERE id = ?2"#)
            .bind(&json)
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn get_by_id(db: &SqlitePool, id: &Uuid) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM users
        WHERE id = ?1
        "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_by_ids(db: &SqlitePool, ids: &[Uuid]) -> Result<Vec<Self>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut results = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(500) {
            let placeholders = chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM users WHERE id IN ({placeholders})");
            let mut q = sqlx::query_as::<_, Self>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            results.extend(
                q.fetch_all(db)
                    .await?,
            );
        }
        Ok(results)
    }

    pub async fn get_by_username(
        db: &SqlitePool,
        username: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM users
        WHERE username = ?1
        "#,
        )
        .bind(username)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub fn new_with_password(
        key: String,
        username: String,
        password: &str,
        aio_url: Option<String>,
    ) -> Result<Self> {
        let password_hash = Self::hash_password(password)?;
        Ok(Self {
            id: get_uuid(),
            username,
            password_hash: password_hash.into(),
            aio_url: aio_url.map(Into::into),
            ..Default::default()
        })
    }

    pub async fn get_by_filter(
        db: &sqlx::SqlitePool,
        filter: &UserFilter,
    ) -> Result<FilterResult<User>> {
        let mut count_qb =
            sqlx::QueryBuilder::new("SELECT COUNT(*) as count FROM users WHERE 1=1");
        let mut records_qb = sqlx::QueryBuilder::new("SELECT * FROM users WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }
            if let Some(username) = &filter.username {
                qb.push(" AND username = ")
                    .push_bind(username);
            }
        }

        if let Some(limit) = &filter.limit {
            records_qb
                .push(" LIMIT ")
                .push_bind(limit);
        }

        if let Some(offset) = &filter.offset {
            records_qb
                .push(" OFFSET ")
                .push_bind(offset);
        }

        let (count, records) = tokio::join!(
            async {
                let query = count_qb.build();
                let row = query
                    .fetch_one(db)
                    .await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<User>();
                query
                    .fetch_all(db)
                    .await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: if filter.total_count { count? } else { 0 },
        })
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        self.password_hash = Self::hash_password(password)?.into();
        Ok(())
    }

    pub fn verify_password(&self, password: &str) -> Result<bool> {
        let parsed = PasswordHash::new(
            self.password_hash
                .expose(),
        )
        .map_err(|e| anyhow!("invalid stored password hash: {e}"))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }

    pub fn hash_password(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("password hashing failed: {e}"))?;

        Ok(hash.to_string())
    }

    pub async fn authenticate(
        db: &SqlitePool,
        username: &str,
        password: &str,
    ) -> Result<Option<Self>> {
        let Some(user) = Self::get_by_username(db, username).await? else {
            return Ok(None);
        };

        if user.verify_password(password)? {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<bool> {
        sqlx::query("DELETE FROM devices WHERE user_id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        // user_media_state is intentionally not cleaned up — see schema comment
        let result = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub fn can_remote_control_others(&self) -> bool {
        self.is_admin
            || self
                .policy
                .as_deref()
                .map_or(false, |p| p.enable_remote_control_of_other_users)
    }

    pub async fn get_media_state(
        &self,
        db: &SqlitePool,
        media: &super::Media,
    ) -> Result<Option<UserMediaState>> {
        Ok(UserMediaState::get_by_user_and_media(db, self, media).await?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CustomData {
    pub id: String,
    // #[serde(with = "serde_json")]
    // pub data: Json
    //pub data: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaIdRaw {
    pub kind: super::MediaKind,
    pub external_ids: super::ExternalIds,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl MediaIdRaw {
    pub fn canonical(&self) -> Option<String> {
        use super::MediaKind;
        match self.kind {
            MediaKind::Movie | MediaKind::Series | MediaKind::TvProgram => self
                .external_ids
                .candidate_ids(&self.kind, None, None, None)
                .into_iter()
                .next(),
            MediaKind::Season | MediaKind::Episode => None,
            MediaKind::Artist => self
                .external_ids
                .deezer_artist
                .map(|id| id.to_string()),
            MediaKind::Album => self
                .external_ids
                .deezer_album
                .map(|id| id.to_string()),
            MediaKind::Track => self
                .external_ids
                .deezer_track
                .map(|id| id.to_string()),
            MediaKind::Person => self
                .external_ids
                .tmdb
                .map(|id| id.to_string()),
            _ => None,
        }
    }
}

impl From<&MediaIdRaw> for Uuid {
    fn from(raw: &MediaIdRaw) -> Uuid {
        crate::common::stable_media_uuid(
            &raw.kind,
            &raw.canonical()
                .unwrap_or_default(),
        )
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserMediaState {
    pub user_id: Uuid,
    pub media_id: Uuid,
    pub media_raw: Option<String>,
    pub stream_id: Option<Uuid>,
    pub favorite: bool,
    pub play_count: i64,
    pub played_at: Option<NaiveDateTime>,
    pub playback_position: i64,
    pub last_played_at: Option<NaiveDateTime>,
    pub subtitle_idx: Option<i64>,
    pub audio_idx: Option<i64>,
    /// Set via [`UserMediaState::set_rating`] so it only holds parsed [`UserRating`]s.
    pub rating: Option<f64>,
}

/// A personal rating on Jellyfin's 0-10 scale. Jellyfin stores no `Likes`
/// field, deriving it from this at [`UserRating::LIKE_THRESHOLD`].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct UserRating(f64);

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum UserRatingError {
    #[error("rating must be a finite number")]
    NotFinite,
    #[error("rating must be between 0 and 10")]
    OutOfRange(f64),
}

impl UserRating {
    /// Jellyfin's `UserItemData.MinLikeValue`.
    pub const LIKE_THRESHOLD: f64 = 6.5;
    pub const MIN: f64 = 0.0;
    pub const MAX: f64 = 10.0;

    /// Jellyfin's `Likes` setter: a like is 10, a dislike is 1.
    pub fn from_likes(likes: bool) -> Self {
        Self(if likes { 10.0 } else { 1.0 })
    }

    pub fn value(self) -> f64 {
        self.0
    }

    pub fn likes(self) -> bool {
        self.0 >= Self::LIKE_THRESHOLD
    }
}

impl TryFrom<f64> for UserRating {
    type Error = UserRatingError;

    fn try_from(value: f64) -> std::result::Result<Self, Self::Error> {
        // NaN needs its own arm: every comparison against it is false, so a
        // bare range check would pass it through to the database.
        if !value.is_finite() {
            return Err(UserRatingError::NotFinite);
        }
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(UserRatingError::OutOfRange(value));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMediaStateFilter {
    pub user_id: Option<Uuid>,
    pub media_id: Option<Vec<Uuid>>,
    pub played: Option<bool>,
    pub favorite: Option<bool>,
    pub resumable: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl UserMediaState {
    pub async fn get_by_user_and_media(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(
            "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(user.id)
        .bind(media.id)
        .fetch_optional(db)
        .await?;

        Ok(row)
    }

    pub async fn get_or_new(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
    ) -> Result<Self> {
        // Include the current UUID plus all stable UUIDs this item could have been
        // stored under (one per external ID). ORDER BY puts the current ID first so
        // no migration fires for the common case.
        let mut all_ids: Vec<Uuid> = Vec::with_capacity(6);
        all_ids.push(media.id);
        all_ids.extend(super::Media::ext_id_uuid_candidates(media));
        // For episodes/seasons the flat candidates are derived from the grandparent
        // series' external IDs, which are usually not preloaded on the media row.
        // Load them so a purged+repopulated library can still find old state rows.
        if matches!(
            media.kind,
            super::MediaKind::Season | super::MediaKind::Episode
        ) && media
            .grandparent
            .is_none()
        {
            if let Some(gp_id) = media
                .grandparent_id
                .or(media.parent_id)
            {
                if let Ok(Some(gp)) = super::Media::get_by_id(db, &gp_id).await {
                    let mut with_gp = media.clone();
                    with_gp.grandparent = Some(Box::new(gp));
                    all_ids.extend(super::Media::ext_id_uuid_candidates(&with_gp));
                }
            }
        }

        let placeholders = all_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT * FROM user_media_state \
             WHERE user_id = ? AND media_id IN ({placeholders}) \
             ORDER BY (media_id = ?) DESC LIMIT 1"
        );
        let mut q = sqlx::query_as::<_, Self>(&sql).bind(user.id);
        for uuid in &all_ids {
            q = q.bind(*uuid);
        }
        q = q.bind(media.id);

        if let Some(mut row) = q
            .fetch_optional(db)
            .await?
        {
            if row.media_id != media.id {
                sqlx::query(
                    "UPDATE user_media_state SET media_id = ? WHERE user_id = ? AND media_id = ?",
                )
                .bind(media.id)
                .bind(user.id)
                .bind(row.media_id)
                .execute(db)
                .await
                .ok();
                row.media_id = media.id;
            }
            return Ok(row);
        }

        Ok(Self {
            user_id: user.id,
            media_id: media.id,
            ..Default::default()
        })
    }

    /// After a media item is (re-)imported, remap any `user_media_state` rows
    /// that are still keyed to an old UUID for that item. Covers all users.
    ///
    /// Useful when the same content is purged and re-imported with a new UUID:
    /// rather than waiting for each user to play the item before their state is
    /// migrated lazily by `get_or_new`, this sweeps the whole table immediately.
    pub async fn remap_orphaned_for(db: &SqlitePool, items: &[super::Media]) {
        for item in items {
            for old_id in super::Media::ext_id_uuid_candidates(item) {
                sqlx::query(
                    "UPDATE user_media_state SET media_id = ? WHERE media_id = ?",
                )
                .bind(item.id)
                .bind(old_id)
                .execute(db)
                .await
                .ok();
            }
        }
    }

    /// Set or clear the personal rating.
    pub async fn set_rating(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
        rating: Option<UserRating>,
    ) -> Result<Self> {
        let mut ms = Self::get_or_new(db, user, media).await?;
        ms.rating = rating.map(UserRating::value);
        ms.save(db)
            .await?;
        Ok(ms)
    }

    /// Jellyfin does not persist `Likes`; it derives it from the rating.
    pub fn likes(&self) -> Option<bool> {
        self.rating
            .map(|r| r >= UserRating::LIKE_THRESHOLD)
    }

    /// Persist playback position (and optionally stream-selection preferences)
    /// for a user/media pair.
    ///
    /// * `position_ticks` – current playback position in 100-nanosecond ticks.
    /// * `audio_idx` / `subtitle_idx` – stream selections to remember; pass
    ///   `None` to leave existing values unchanged.
    /// * `runtime_seconds` – when `Some`, the 90 % "mark as watched" threshold
    ///   is applied. Pass `None` for progress updates (no watched-check) and
    ///   `Some(media.runtime)` for stop events.
    ///
    /// Returns whether this report crossed the played threshold, which is
    /// always `false` when `runtime_seconds` is `None` because no threshold
    /// was applied.
    pub async fn update_playback(
        db: &SqlitePool,
        user: &User,
        media: &super::Media,
        position_ticks: i64,
        audio_idx: Option<i64>,
        subtitle_idx: Option<i64>,
        runtime_seconds: Option<i64>,
    ) -> Result<bool> {
        let mut ms = Self::get_or_new(db, user, media).await?;
        let position_seconds = position_ticks / 10_000_000;
        ms.playback_position = position_seconds;

        if let Some(idx) = audio_idx {
            ms.audio_idx = Some(idx);
        }
        if let Some(idx) = subtitle_idx {
            ms.subtitle_idx = Some(idx);
        }

        // On stop events apply resume/played thresholds from server config.
        let crossed_played_threshold = if let Some(runtime) = runtime_seconds {
            let server_config = Settings::get_config_or_default(db).await;
            let min_pct = server_config
                .min_resume_pct
                .unwrap_or(5);
            let max_pct = server_config
                .max_resume_pct
                .unwrap_or(90);
            let min_duration = server_config
                .min_resume_duration_seconds
                .unwrap_or(90);

            let played = runtime > 0 && position_seconds >= runtime * max_pct / 100;
            let no_resume = runtime > 0
                && (runtime < min_duration
                    || position_seconds < runtime * min_pct / 100);

            if played {
                ms.playback_position = 0;
                ms.save(db)
                    .await?;
                media
                    .mark_played(db, user, true, server_config.release_date_threshold())
                    .await?;
                sqlx::query(
                    "UPDATE user_media_state SET playback_position = 0 \
                     WHERE user_id = ? AND media_id = ?",
                )
                .bind(user.id)
                .bind(media.id)
                .execute(db)
                .await?;
            } else if no_resume {
                ms.playback_position = 0;
                ms.save(db)
                    .await?;
            } else {
                ms.save(db)
                    .await?;
            }
            played
        } else {
            ms.save(db)
                .await?;
            false
        };

        Ok(crossed_played_threshold)
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        debug!(
            "Saving user media state for user {} and media_id {}",
            self.user_id, self.media_id
        );

        let now = chrono::Utc::now().naive_utc();
        sqlx::query(
            r#"
            INSERT INTO user_media_state (
                user_id,
                media_id,
                media_raw,
                stream_id,
                favorite,
                play_count,
                played_at,
                playback_position,
                last_played_at,
                subtitle_idx,
                audio_idx,
                rating
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(user_id, media_id)
            DO UPDATE SET
                media_raw = excluded.media_raw,
                stream_id = excluded.stream_id,
                favorite = excluded.favorite,
                play_count = excluded.play_count,
                played_at = excluded.played_at,
                playback_position = excluded.playback_position,
                last_played_at = excluded.last_played_at,
                subtitle_idx = excluded.subtitle_idx,
                audio_idx = excluded.audio_idx,
                rating = excluded.rating
            "#,
        )
        .bind(self.user_id)
        .bind(self.media_id)
        .bind(&self.media_raw)
        .bind(self.stream_id)
        .bind(self.favorite)
        .bind(self.play_count)
        .bind(self.played_at)
        .bind(self.playback_position)
        .bind(now)
        .bind(self.subtitle_idx)
        .bind(self.audio_idx)
        .bind(self.rating)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_filter(
        db: &SqlitePool,
        filter: &UserMediaStateFilter,
    ) -> Result<FilterResult<Self>> {
        let mut count_qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*) as count FROM user_media_state WHERE 1=1",
        );
        let mut records_qb =
            sqlx::QueryBuilder::new("SELECT * FROM user_media_state WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(user_id) = &filter.user_id {
                qb.push(" AND user_id = ")
                    .push_bind(user_id);
            }
            if let Some(media_ids) = &filter.media_id {
                qb.push_in("media_id", &media_ids);
            }
            if let Some(played) = &filter.played {
                qb.push(" AND play_count > 0");
            }
            if let Some(favorite) = &filter.favorite {
                qb.push(" AND favorite = ")
                    .push_bind(favorite);
            }
        }

        if let Some(limit) = &filter.limit {
            records_qb
                .push(" LIMIT ")
                .push_bind(limit);
        }
        if let Some(offset) = &filter.offset {
            records_qb
                .push(" OFFSET ")
                .push_bind(offset);
        }

        let (count, records) = tokio::join!(
            async {
                let query = count_qb.build();
                let row = query
                    .fetch_one(db)
                    .await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<UserMediaState>();
                query
                    .fetch_all(db)
                    .await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: count?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HomeSection {
    pub order: i64,
    pub kind: String,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefsData {
    pub view_type: Option<String>,
    pub sort_by: Option<String>,
    pub index_by: Option<String>,
    #[default(false)]
    pub remember_indexing: bool,
    #[default(250)]
    pub primary_image_height: i64,
    #[default(250)]
    pub primary_image_width: i64,
    #[serde(default)]
    pub custom_prefs: HashMap<String, Option<String>>,
    #[default(ScrollDirection::Horizontal)]
    pub scroll_direction: ScrollDirection,
    #[default(true)]
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    #[default(SortOrder::Ascending)]
    pub sort_order: SortOrder,
    pub show_sidebar: bool,
    pub home_sections: Option<Vec<HomeSection>>,
}

pub fn default_homescreen_custom_prefs() -> HashMap<String, Option<String>> {
    [
        ("homesection0", "smalllibrarytiles"),
        ("homesection1", "resume"),
        ("homesection2", "nextup"),
        ("homesection3", "latestmedia"),
        ("homesection4", "livetv"),
        ("homesection5", "none"),
        ("homesection6", "none"),
        ("homesection7", "none"),
        ("homesection8", "none"),
        ("homesection9", "none"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Some(v.to_string())))
    .collect()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefs {
    pub id: String,
    pub user_id: Uuid,
    pub client: Option<String>,
    pub data: sqlx::types::Json<JellyfinDisplayPrefsData>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct JellyfinDisplayPrefsFilter {
    pub id: Option<Vec<String>>,
    pub user_id: Option<Uuid>,
    pub client: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub total_count: bool,
}

impl JellyfinDisplayPrefs {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jellyfin_display_prefs (id, user_id, client, data)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                user_id = excluded.user_id,
                client  = excluded.client,
                data    = excluded.data
            "#,
        )
        .bind(&self.id)
        .bind(self.user_id)
        .bind(&self.client)
        .bind(&self.data)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_filter(
        db: &sqlx::SqlitePool,
        filter: &JellyfinDisplayPrefsFilter,
    ) -> Result<FilterResult<Self>> {
        let mut count_qb = sqlx::QueryBuilder::new(
            "SELECT COUNT(*) as count FROM jellyfin_display_prefs WHERE 1=1",
        );
        let mut records_qb =
            sqlx::QueryBuilder::new("SELECT * FROM jellyfin_display_prefs WHERE 1=1");

        for qb in [&mut count_qb, &mut records_qb] {
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }
            if let Some(client) = &filter.client {
                qb.push(" AND client = ")
                    .push_bind(client);
            }
            if let Some(user_id) = &filter.user_id {
                qb.push(" AND user_id = ")
                    .push_bind(user_id);
            }
        }

        if let Some(limit) = &filter.limit {
            records_qb
                .push(" LIMIT ")
                .push_bind(limit);
        }

        if let Some(offset) = &filter.offset {
            records_qb
                .push(" OFFSET ")
                .push_bind(offset);
        }

        let (count, records) = tokio::join!(
            async {
                let query = count_qb.build();
                let row = query
                    .fetch_one(db)
                    .await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<Self>();
                query
                    .fetch_all(db)
                    .await
            }
        );

        Ok(FilterResult {
            records: records?,
            total_count: if filter.total_count { count? } else { 0 },
        })
    }
}

/// Resolves the target user from `user_id` path param or `userId` query param.
/// Falls back to the session user when neither is present.
/// Admins may target any user; non-admins may only target themselves.
impl FromRequestParts<crate::AppState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        use crate::db::auth::AuthSession;
        use axum::extract::Path;

        let session = AuthSession::from_request_parts(parts, state).await?;

        let user_id = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .ok()
            .and_then(|Path(p)| {
                p.get("user_id")
                    .and_then(|s| Uuid::parse_str(s).ok())
            })
            .or_else(|| {
                parts
                    .uri
                    .query()
                    .and_then(|q| {
                        serde_urlencoded::from_str::<HashMap<String, String>>(q).ok()
                    })
                    .and_then(|m| {
                        m.get("userId")
                            .and_then(|s| Uuid::parse_str(s).ok())
                    })
            })
            .unwrap_or(
                session
                    .user
                    .id,
            );

        if user_id
            == session
                .user
                .id
        {
            return Ok(session.user);
        }

        if !session
            .user
            .is_admin
        {
            return Err(anyhow!("Forbidden").context_forbidden("Forbidden"));
        }

        User::get_by_id(
            &state
                .ctx
                .db,
            &user_id,
        )
        .await
        .map_err(|e| anyhow!(e).context_internal("db error"))?
        .context_not_found("user not found")
    }
}

#[cfg(test)]
mod rating_tests {
    use super::*;

    /// The representable value immediately below the threshold, so the `>=`
    /// is pinned rather than merely exercised.
    fn just_under_threshold() -> f64 {
        UserRating::LIKE_THRESHOLD.next_down()
    }

    #[test]
    fn the_range_bounds_are_inclusive() {
        for v in [
            UserRating::MIN,
            0.5,
            UserRating::LIKE_THRESHOLD,
            9.5,
            UserRating::MAX,
        ] {
            assert_eq!(
                UserRating::try_from(v).map(UserRating::value),
                Ok(v),
                "{v} should be accepted"
            );
        }
    }

    #[test]
    fn values_outside_the_range_are_rejected() {
        for v in [
            UserRating::MIN.next_down(),
            -1.0,
            UserRating::MAX.next_up(),
            11.0,
            f64::MIN,
            f64::MAX,
        ] {
            assert_eq!(
                UserRating::try_from(v),
                Err(UserRatingError::OutOfRange(v)),
                "{v} should be rejected"
            );
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        for v in [f64::NAN, -f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                UserRating::try_from(v),
                Err(UserRatingError::NotFinite),
                "{v} should be rejected"
            );
        }
    }

    #[test]
    fn negative_zero_is_in_range() {
        let r = UserRating::try_from(-0.0).unwrap();
        assert_eq!(r.value(), 0.0);
        assert!(!r.likes());
    }

    #[test]
    fn likes_flips_at_the_threshold() {
        assert!(
            !UserRating::try_from(UserRating::MIN)
                .unwrap()
                .likes()
        );
        assert!(
            !UserRating::try_from(just_under_threshold())
                .unwrap()
                .likes()
        );
        assert!(
            UserRating::try_from(UserRating::LIKE_THRESHOLD)
                .unwrap()
                .likes()
        );
        assert!(
            UserRating::try_from(UserRating::MAX)
                .unwrap()
                .likes()
        );
    }

    #[test]
    fn the_likes_shorthand_writes_jellyfins_values() {
        assert_eq!(UserRating::from_likes(true).value(), 10.0);
        assert_eq!(UserRating::from_likes(false).value(), 1.0);
        assert!(UserRating::from_likes(true).likes());
        assert!(!UserRating::from_likes(false).likes());
    }

    #[test]
    fn likes_is_derived_from_the_stored_column() {
        let state = |rating| UserMediaState {
            rating,
            ..Default::default()
        };
        assert_eq!(state(None).likes(), None);
        assert_eq!(state(Some(just_under_threshold())).likes(), Some(false));
        assert_eq!(state(Some(UserRating::LIKE_THRESHOLD)).likes(), Some(true));
    }
}

#[cfg(test)]
mod playback_threshold_tests {
    use super::*;
    use crate::{db, integration_test::new_test_server};

    const RUNTIME: i64 = 6_000;

    fn ticks(seconds: i64) -> i64 {
        seconds * 10_000_000
    }

    async fn movie(ctx: &crate::AppContext) -> db::Media {
        let external_ids = db::ExternalIds {
            imdb: db::NonEmptyString::try_new("tt0113277".to_string()).ok(),
            tmdb: Some(949),
            ..Default::default()
        };
        let mut m = db::Media {
            id: uuid::Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Movie,
                external_ids: external_ids.clone(),
                season: None,
                episode: None,
            }),
            title: "Heat".into(),
            kind: db::MediaKind::Movie,
            runtime: Some(RUNTIME),
            external_ids,
            ..Default::default()
        };
        m.save(&ctx.db)
            .await
            .unwrap();
        m
    }

    async fn stop_at(
        ctx: &crate::AppContext,
        user: &User,
        media: &db::Media,
        secs: i64,
    ) -> bool {
        UserMediaState::update_playback(
            &ctx.db,
            user,
            media,
            ticks(secs),
            None,
            None,
            media.runtime,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_rewatch_that_stops_early_is_not_a_second_watch() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let user = db::User::get_by_username(&ctx.db, "test")
            .await
            .unwrap()
            .unwrap();
        let media = movie(ctx).await;

        assert!(
            stop_at(ctx, &user, &media, RUNTIME * 95 / 100).await,
            "stopping past the threshold is a watch"
        );

        assert!(
            !stop_at(ctx, &user, &media, RUNTIME * 10 / 100).await,
            "stopping early is not a watch, even once the item is already played"
        );

        // The distinction only exists while `played_at` stays set, so a cleared
        // one would make the assertion above pass for the wrong reason.
        assert!(
            UserMediaState::get_or_new(&ctx.db, &user, &media)
                .await
                .unwrap()
                .played_at
                .is_some(),
            "the earlier watch should still stand"
        );
    }

    #[tokio::test]
    async fn a_progress_report_never_claims_a_threshold_it_did_not_check() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let user = db::User::get_by_username(&ctx.db, "test")
            .await
            .unwrap()
            .unwrap();
        let media = movie(ctx).await;

        stop_at(ctx, &user, &media, RUNTIME * 95 / 100).await;

        let crossed = UserMediaState::update_playback(
            &ctx.db,
            &user,
            &media,
            ticks(RUNTIME * 99 / 100),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(!crossed, "no runtime means no threshold was applied");
    }
}
