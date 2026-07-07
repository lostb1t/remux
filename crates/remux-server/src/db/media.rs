use super::{FilterResult, ImageKind, MediaImage, MediaImages, QueryBuilderExt};

pub const CHUNK_SIZE: usize = 250;
const SQLITE_VAR_LIMIT: usize = 999;

static DB_WRITE_SEMAPHORE: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(1));
use crate::{
    OptionExt, ResultExt, api,
    api::MediaSourceInfo,
    common::{IntoVec, get_uuid, server_id},
    sdks,
    services::stremio as stremio_service,
    stream::{StreamDescriptor, StreamInfo},
};
use anyhow::{Context, Result, anyhow};
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
use chrono::{DateTime, Duration, NaiveDateTime, Utc, prelude::*};
use config::{self, Config};
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use regex::Regex;
use reqwest::{self, header::LOCATION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_with::skip_serializing_none;
use sqlx::{Row, SqlitePool};
use std::{
    self,
    collections::HashMap,
    env, fs,
    path::Path,
    str::FromStr,
    sync::{Arc, LazyLock},
};
use thiserror::Error;
use timed;
use tower::{Layer, util::MapRequestLayer};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};
use tracing::{self, debug, error, info, instrument, trace, warn};
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use url::Url;
use uuid::{Uuid, uuid};

#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum ProgramKind {
    Movie,
    Series,
    News,
    Kids,
    Sports,
}

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum MediaStatus {
    Continuing,
    Ended,
    Unreleased,
    Released,
    #[default]
    Unknown,
}

#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
//#[sqlx(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Series,
    Season,
    Episode,
    Person,
    Studio,
    Genre,
    Country,
    MusicGenre,
    Collection,
    // purely here for jf
    Folder,
    Stream,
    TvChannel,
    TvProgram,
    // Music
    Track,
    Album,
    Artist,
    Playlist,
    StreamGroup,
    Subtitle,
    Intro,
}

impl MediaKind {
    pub fn is_folder(&self) -> bool {
        matches!(
            self,
            Self::Series
                | Self::Collection
                | Self::Season
                | Self::Folder
                | Self::Playlist
                | Self::Album
                | Self::Artist
        )
    }
}

impl TryFrom<String> for MediaKind {
    type Error = strum::ParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<sdks::stremio::MediaType> for MediaKind {
    type Error = ();

    fn try_from(t: sdks::stremio::MediaType) -> Result<Self, Self::Error> {
        match t {
            sdks::stremio::MediaType::Movie => Ok(MediaKind::Movie),
            sdks::stremio::MediaType::Series => Ok(MediaKind::Series),
            sdks::stremio::MediaType::Tv | sdks::stremio::MediaType::Channel => {
                Ok(MediaKind::TvChannel)
            }
            sdks::stremio::MediaType::Album => Ok(MediaKind::Album),
            sdks::stremio::MediaType::Artist => Ok(MediaKind::Artist),
            sdks::stremio::MediaType::Track => Ok(MediaKind::Track),
            sdks::stremio::MediaType::Events => Ok(MediaKind::TvProgram),
            sdks::stremio::MediaType::Unknown(s) => match s.as_str() {
                "episode" => Ok(MediaKind::Episode),
                "season" => Ok(MediaKind::Season),
                "person" => Ok(MediaKind::Person),
                _ => Err(()),
            },
        }
    }
}

impl From<&MediaKind> for sdks::stremio::MediaType {
    fn from(kind: &MediaKind) -> Self {
        match kind {
            MediaKind::Movie => sdks::stremio::MediaType::Movie,
            MediaKind::Series | MediaKind::Season | MediaKind::Episode => {
                sdks::stremio::MediaType::Series
            }
            MediaKind::TvChannel | MediaKind::TvProgram => sdks::stremio::MediaType::Tv,
            MediaKind::Track => sdks::stremio::MediaType::Track,
            MediaKind::Album => sdks::stremio::MediaType::Album,
            MediaKind::Artist => sdks::stremio::MediaType::Artist,
            _ => sdks::stremio::MediaType::Movie,
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<sdks::remux::MediaKind> for MediaKind {
    fn into(self) -> sdks::remux::MediaKind {
        match self {
            MediaKind::Movie => sdks::remux::MediaKind::Movie,
            MediaKind::Series => sdks::remux::MediaKind::Series,
            MediaKind::Season => sdks::remux::MediaKind::Season,
            MediaKind::Episode => sdks::remux::MediaKind::Episode,
            MediaKind::Collection => sdks::remux::MediaKind::Collection,
            MediaKind::Folder => sdks::remux::MediaKind::Folder,
            MediaKind::Genre | MediaKind::MusicGenre => sdks::remux::MediaKind::Genre,
            MediaKind::Person => sdks::remux::MediaKind::Person,
            MediaKind::Studio | MediaKind::Country => sdks::remux::MediaKind::Studio,
            MediaKind::Stream => sdks::remux::MediaKind::Stream,
            MediaKind::TvChannel => sdks::remux::MediaKind::TvChannel,
            MediaKind::TvProgram => sdks::remux::MediaKind::TvProgram,
            MediaKind::Track => sdks::remux::MediaKind::Track,
            MediaKind::Album => sdks::remux::MediaKind::Album,
            MediaKind::Artist => sdks::remux::MediaKind::Artist,
            MediaKind::Playlist => sdks::remux::MediaKind::Playlist,
            MediaKind::StreamGroup => sdks::remux::MediaKind::Stream,
            MediaKind::Subtitle => sdks::remux::MediaKind::Stream,
            MediaKind::Intro => sdks::remux::MediaKind::Stream,
        }
    }
}

impl From<sdks::remux::MediaKind> for MediaKind {
    fn from(k: sdks::remux::MediaKind) -> Self {
        match k {
            sdks::remux::MediaKind::Movie => MediaKind::Movie,
            sdks::remux::MediaKind::Series => MediaKind::Series,
            sdks::remux::MediaKind::Mixed => MediaKind::Collection,
            sdks::remux::MediaKind::Season => MediaKind::Season,
            sdks::remux::MediaKind::Episode => MediaKind::Episode,
            sdks::remux::MediaKind::Collection => MediaKind::Collection,
            sdks::remux::MediaKind::Folder => MediaKind::Folder,
            sdks::remux::MediaKind::Genre => MediaKind::Genre,
            sdks::remux::MediaKind::Person => MediaKind::Person,
            sdks::remux::MediaKind::Studio => MediaKind::Studio,
            sdks::remux::MediaKind::Stream => MediaKind::Stream,
            sdks::remux::MediaKind::TvChannel => MediaKind::TvChannel,
            sdks::remux::MediaKind::TvProgram => MediaKind::TvProgram,
            sdks::remux::MediaKind::Track => MediaKind::Track,
            sdks::remux::MediaKind::Album => MediaKind::Album,
            sdks::remux::MediaKind::Artist => MediaKind::Artist,
            sdks::remux::MediaKind::Playlist => MediaKind::Playlist,
        }
    }
}

impl TryFrom<api::MediaType> for MediaKind {
    type Error = ();
    fn try_from(media_type: api::MediaType) -> Result<Self, ()> {
        match media_type {
            api::MediaType::Movie => Ok(MediaKind::Movie),
            api::MediaType::Series => Ok(MediaKind::Series),
            api::MediaType::Season => Ok(MediaKind::Season),
            api::MediaType::Episode => Ok(MediaKind::Episode),
            api::MediaType::BoxSet => Ok(MediaKind::Collection),
            api::MediaType::TvChannel | api::MediaType::LiveTvChannel => {
                Ok(MediaKind::TvChannel)
            }
            api::MediaType::TvProgram
            | api::MediaType::LiveTvProgram
            | api::MediaType::Program => Ok(MediaKind::TvProgram),
            api::MediaType::Folder
            | api::MediaType::CollectionFolder
            | api::MediaType::UserView
            | api::MediaType::UserRootFolder => Ok(MediaKind::Folder),
            api::MediaType::Genre => Ok(MediaKind::Genre),
            api::MediaType::MusicGenre => Ok(MediaKind::MusicGenre),
            api::MediaType::Person => Ok(MediaKind::Person),
            api::MediaType::Studio => Ok(MediaKind::Studio),
            api::MediaType::Audio => Ok(MediaKind::Track),
            api::MediaType::MusicAlbum => Ok(MediaKind::Album),
            api::MediaType::MusicArtist => Ok(MediaKind::Artist),
            api::MediaType::Playlist => Ok(MediaKind::Playlist),
            _ => Err(()),
        }
    }
}

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum CollectionKind {
    #[default]
    Manual,
    Smart,
    Catalog,
}

impl TryFrom<String> for CollectionKind {
    type Error = strum::ParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

/// What kind of content a Collection/library holds.
/// Stored as TEXT in the DB (snake_case).
#[derive(
    Default,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum CollectionMediaKind {
    #[default]
    Movie,
    Series,
    Mixed,
    Music,
    Collection,
    Playlist,
}

impl From<&str> for CollectionMediaKind {
    fn from(s: &str) -> Self {
        match s
            .trim()
            .to_lowercase()
            .as_str()
        {
            "series" | "episode" => Self::Series,
            "album" | "artist" | "track" => Self::Music,
            _ => Self::Movie,
        }
    }
}

impl From<String> for CollectionMediaKind {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum RelationRole {
    #[default]
    Actor,
    Director,
    Writer,
    Producer,
    Creator,
    Catalog,
    Playlist,
    Collection,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaRelation {
    #[default(get_uuid())]
    pub relation_id: Uuid,
    pub left_media_id: Uuid,
    pub right_media_id: Uuid,
    pub weight: Option<i64>,
    pub role: Option<RelationRole>,
    pub character: Option<String>,
}

impl MediaRelation {
    pub async fn upsert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        let mut tx = db
            .begin()
            .await?;

        for chunk in items.chunks(CHUNK_SIZE) {
            let mut qb = sqlx::QueryBuilder::new(
                "INSERT INTO media_relations (relation_id, left_media_id, right_media_id, weight, role, character) ",
            );

            qb.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.relation_id)
                    .push_bind(&item.left_media_id)
                    .push_bind(&item.right_media_id)
                    .push_bind(&item.weight)
                    .push_bind(&item.role)
                    .push_bind(&item.character);
            });

            qb.push(" ON CONFLICT (left_media_id, right_media_id, COALESCE(role, '')) DO UPDATE SET weight = excluded.weight, character = excluded.character");

            qb.build()
                .execute(&mut *tx)
                .await?;
        }

        tx.commit()
            .await?;
        Ok(())
    }

    pub async fn get_by_media_id(
        db: &SqlitePool,
        media_id: &Uuid,
    ) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Self>(
            "SELECT * FROM media_relations WHERE left_media_id = $1 ORDER BY weight ASC",
        )
        .bind(media_id)
        .fetch_all(db)
        .await?;

        Ok(rows)
    }

    pub async fn delete_by_left_id(
        db: &SqlitePool,
        left_media_id: &Uuid,
    ) -> Result<()> {
        sqlx::query("DELETE FROM media_relations WHERE left_media_id = ?")
            .bind(left_media_id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn delete_by_left_ids(db: &SqlitePool, ids: &[Uuid]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        for chunk in ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media_relations WHERE left_media_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            qb.build()
                .execute(db)
                .await?;
        }
        Ok(())
    }

    pub async fn get_playlist_items(
        db: &SqlitePool,
        playlist_id: &Uuid,
    ) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Self>(
            "SELECT * FROM media_relations WHERE left_media_id = ? AND role = 'playlist' ORDER BY weight ASC",
        )
        .bind(playlist_id)
        .fetch_all(db)
        .await?;
        Ok(rows)
    }

    pub async fn add_playlist_items(
        db: &SqlitePool,
        playlist_id: &Uuid,
        media_ids: &[Uuid],
    ) -> Result<()> {
        if media_ids.is_empty() {
            return Ok(());
        }
        let max_weight: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(weight) FROM media_relations WHERE left_media_id = ? AND role = 'playlist'",
        )
        .bind(playlist_id)
        .fetch_one(db)
        .await?;
        let mut next_weight = max_weight
            .map(|w| w + 1)
            .unwrap_or(0);
        let items: Vec<Self> = media_ids
            .iter()
            .map(|&media_id| {
                let item = Self {
                    left_media_id: *playlist_id,
                    right_media_id: media_id,
                    weight: Some(next_weight),
                    role: Some(RelationRole::Playlist),
                    ..Default::default()
                };
                next_weight += 1;
                item
            })
            .collect();
        Self::upsert(db, &items).await?;
        sync_playlist_media_kind(db, playlist_id).await;
        Ok(())
    }

    pub async fn delete_by_relation_ids(
        db: &SqlitePool,
        relation_ids: &[Uuid],
    ) -> Result<()> {
        if relation_ids.is_empty() {
            return Ok(());
        }
        let mut qb = sqlx::QueryBuilder::new(
            "DELETE FROM media_relations WHERE relation_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in relation_ids {
            sep.push_bind(id);
        }
        qb.push(")");
        qb.build()
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn delete_by_right_kinds(
        db: &SqlitePool,
        left_id: Uuid,
        right_kinds: &[MediaKind],
    ) -> Result<()> {
        if right_kinds.is_empty() {
            return Ok(());
        }
        let mut qb = sqlx::QueryBuilder::new(
            "DELETE FROM media_relations WHERE left_media_id = ",
        );
        qb.push_bind(left_id);
        qb.push(" AND right_media_id IN (SELECT id FROM media WHERE kind IN (");
        let mut sep = qb.separated(", ");
        for k in right_kinds {
            sep.push_bind(k.to_string());
        }
        qb.push("))");
        qb.build()
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn move_playlist_item(
        db: &SqlitePool,
        playlist_id: &Uuid,
        relation_id: &Uuid,
        new_index: usize,
    ) -> Result<()> {
        let mut items = Self::get_playlist_items(db, playlist_id).await?;
        let Some(pos) = items
            .iter()
            .position(|r| &r.relation_id == relation_id)
        else {
            return Ok(());
        };
        let item = items.remove(pos);
        let insert_at = new_index.min(items.len());
        items.insert(insert_at, item);

        let mut tx = db
            .begin()
            .await?;
        for (i, r) in items
            .iter()
            .enumerate()
        {
            sqlx::query("UPDATE media_relations SET weight = ? WHERE relation_id = ?")
                .bind(i as i64)
                .bind(r.relation_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Manual collection item helpers (same pattern as playlist, role = 'collection')
    // ---------------------------------------------------------------------------

    pub async fn get_collection_items(
        db: &SqlitePool,
        collection_id: &Uuid,
    ) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM media_relations \
             WHERE left_media_id = ? AND role = 'collection' ORDER BY weight ASC",
        )
        .bind(collection_id)
        .fetch_all(db)
        .await?)
    }

    pub async fn add_collection_items(
        db: &SqlitePool,
        collection_id: &Uuid,
        media_ids: &[Uuid],
    ) -> Result<()> {
        if media_ids.is_empty() {
            return Ok(());
        }
        let max_weight: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(weight) FROM media_relations \
             WHERE left_media_id = ? AND role = 'collection'",
        )
        .bind(collection_id)
        .fetch_one(db)
        .await?;
        let mut next_weight = max_weight
            .map(|w| w + 1)
            .unwrap_or(0);
        let items: Vec<Self> = media_ids
            .iter()
            .map(|&media_id| {
                let item = Self {
                    left_media_id: *collection_id,
                    right_media_id: media_id,
                    weight: Some(next_weight),
                    role: Some(RelationRole::Collection),
                    ..Default::default()
                };
                next_weight += 1;
                item
            })
            .collect();
        Self::upsert(db, &items).await
    }

    pub async fn move_collection_item(
        db: &SqlitePool,
        collection_id: &Uuid,
        relation_id: &Uuid,
        new_index: usize,
    ) -> Result<()> {
        let mut items = Self::get_collection_items(db, collection_id).await?;
        let Some(pos) = items
            .iter()
            .position(|r| &r.relation_id == relation_id)
        else {
            return Ok(());
        };
        let item = items.remove(pos);
        let insert_at = new_index.min(items.len());
        items.insert(insert_at, item);

        let mut tx = db
            .begin()
            .await?;
        for (i, r) in items
            .iter()
            .enumerate()
        {
            sqlx::query("UPDATE media_relations SET weight = ? WHERE relation_id = ?")
                .bind(i as i64)
                .bind(r.relation_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await?;
        Ok(())
    }

    /// Replace all items in a manual collection with the given ordered list.
    /// Used by catalog import — clears existing items and inserts fresh ones.
    pub async fn replace_collection_items(
        db: &SqlitePool,
        collection_id: &Uuid,
        media_ids: &[Uuid],
    ) -> Result<()> {
        let mut tx = db
            .begin()
            .await?;
        sqlx::query(
            "DELETE FROM media_relations WHERE left_media_id = ? AND role = 'collection'",
        )
        .bind(collection_id)
        .execute(&mut *tx)
        .await?;
        tx.commit()
            .await?;

        Self::add_collection_items(db, collection_id, media_ids).await
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rating {
    pub score: f64,
    pub vote_count: Option<u32>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalRatings {
    pub tmdb: Option<Rating>,
}

impl ExternalRatings {
    pub fn audience_rating(&self) -> Option<f64> {
        const PRIOR: f64 = 6.5;
        const M: f64 = 500.0;

        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        if let Some(r) = &self.tmdb {
            let v = r
                .vote_count
                .unwrap_or(0) as f64;
            let bayesian = (v / (v + M)) * r.score + (M / (v + M)) * PRIOR;
            weighted_sum += bayesian * 1.0;
            total_weight += 1.0;
        }

        (total_weight > 0.0).then(|| weighted_sum / total_weight)
    }
}

pub use remux_utils::NonEmptyString;

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalIds {
    pub imdb: Option<NonEmptyString>,
    pub series_imdb: Option<NonEmptyString>,
    pub tmdb: Option<i64>,
    pub series_tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub deezer_artist: Option<i64>,
    pub deezer_album: Option<i64>,
    pub deezer_track: Option<i64>,
    pub deezer_playlist: Option<i64>,
    pub youtube_id: Option<String>,
    pub iptv_source_id: Option<String>,
    pub iptv_group: Option<String>,
    /// Raw addon-specific ID for content that has no IMDB/TMDB/TVDB equivalent.
    /// Derived from the Stremio `meta.id` when no known provider prefix matches.
    pub custom_stremio_id: Option<String>,
    /// For seasons/episodes of a custom-ID series: the parent series's `custom_stremio_id`.
    /// Analogous to `series_imdb` for the custom-ID path.
    pub series_custom_stremio_id: Option<String>,
}

impl ExternalIds {
    /// Parse an AIO `meta.id` string into external provider IDs using the
    /// standard Stremio/Jellyfin prefix conventions.
    pub fn from_stremio_id(id: &str) -> Self {
        if id.starts_with("tt") {
            return Self {
                imdb: NonEmptyString::try_new(id.to_string()).ok(),
                ..Default::default()
            };
        }
        if let Some(rest) = id.strip_prefix("tmdb:") {
            if let Ok(n) = rest.parse::<i64>() {
                return Self {
                    tmdb: Some(n),
                    ..Default::default()
                };
            }
        }
        if let Some(rest) = id.strip_prefix("tvdb:") {
            if let Ok(n) = rest.parse::<i64>() {
                return Self {
                    tvdb: Some(n),
                    ..Default::default()
                };
            }
        }
        if !id.is_empty() {
            return Self {
                custom_stremio_id: Some(id.to_string()),
                ..Default::default()
            };
        }
        Self::default()
    }

    /// Parse Jellyfin metadata provider IDs from a file path.
    ///
    /// Scans all path components for bracket-encoded provider IDs, e.g.
    /// `Movies/The Matrix (1999) [tmdbid-603]/The Matrix.mkv` → `tmdb: Some(603)`.
    /// Supported providers (case-insensitive): tmdbid/tmdb, imdbid/imdb, tvdbid/tvdb.
    pub fn from_path(path: &str) -> Self {
        static RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)\[(tmdb(?:id)?|imdb(?:id)?|tvdb(?:id)?)-([^\]]+)\]")
                .unwrap()
        });
        let mut result = Self::default();
        for cap in RE.captures_iter(path) {
            let provider = cap[1].to_ascii_lowercase();
            let value = cap[2]
                .trim()
                .to_string();
            match provider.as_str() {
                "tmdb" | "tmdbid" => {
                    if result
                        .tmdb
                        .is_none()
                    {
                        result.tmdb = value
                            .parse::<i64>()
                            .ok();
                    }
                }
                "imdb" | "imdbid" => {
                    if result
                        .imdb
                        .is_none()
                    {
                        result.imdb = NonEmptyString::try_new(value).ok();
                    }
                }
                "tvdb" | "tvdbid" => {
                    if result
                        .tvdb
                        .is_none()
                    {
                        result.tvdb = value
                            .parse::<i64>()
                            .ok();
                    }
                }
                _ => {}
            }
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        self.imdb
            .is_none()
            && self
                .series_imdb
                .is_none()
            && self
                .tmdb
                .is_none()
            && self
                .tvdb
                .is_none()
            && self
                .custom_stremio_id
                .is_none()
            && self
                .series_custom_stremio_id
                .is_none()
    }

    /// Returns the best Stremio ID for use as a lookup key or idPrefix match.
    /// Priority: series_imdb → imdb → tmdb:{n} → series_custom_stremio_id → custom_stremio_id
    pub fn stremio_lookup_id(&self) -> Option<String> {
        self.series_imdb
            .as_deref()
            .map(|s| s.to_string())
            .or_else(|| {
                self.imdb
                    .as_deref()
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                self.tmdb
                    .map(|n| format!("tmdb:{}", n))
            })
            .or_else(|| {
                self.series_custom_stremio_id
                    .clone()
            })
            .or_else(|| {
                self.custom_stremio_id
                    .clone()
            })
    }

    pub fn merge(&mut self, source: &Self, replace: bool) {
        use remux_utils::merge_option;
        merge_option(&mut self.imdb, &source.imdb, replace);
        merge_option(&mut self.series_imdb, &source.series_imdb, replace);
        merge_option(&mut self.tmdb, &source.tmdb, replace);
        merge_option(&mut self.series_tmdb, &source.series_tmdb, replace);
        merge_option(&mut self.tvdb, &source.tvdb, replace);
        merge_option(&mut self.deezer_artist, &source.deezer_artist, replace);
        merge_option(&mut self.deezer_album, &source.deezer_album, replace);
        merge_option(&mut self.deezer_track, &source.deezer_track, replace);
        merge_option(&mut self.deezer_playlist, &source.deezer_playlist, replace);
        merge_option(&mut self.youtube_id, &source.youtube_id, replace);
        merge_option(&mut self.iptv_source_id, &source.iptv_source_id, replace);
        merge_option(&mut self.iptv_group, &source.iptv_group, replace);
        merge_option(
            &mut self.custom_stremio_id,
            &source.custom_stremio_id,
            replace,
        );
        merge_option(
            &mut self.series_custom_stremio_id,
            &source.series_custom_stremio_id,
            replace,
        );
    }
}

/// Update a playlist's `collection_media_kind` based on its first item's kind.
/// Called after items are added or removed so the playlist's `MediaType` stays accurate.
pub async fn sync_playlist_media_kind(db: &SqlitePool, playlist_id: &Uuid) {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT m.kind FROM media_relations mr \
         JOIN media m ON m.id = mr.right_media_id \
         WHERE mr.left_media_id = ? AND mr.role = 'playlist' \
         ORDER BY mr.weight ASC LIMIT 1",
    )
    .bind(playlist_id)
    .fetch_optional(db)
    .await
    .unwrap_or(None);

    let media_kind = match kind.as_deref() {
        Some("track") | Some("album") | Some("artist") => "music",
        Some(_) => "movie",
        None => return,
    };

    sqlx::query(
        "UPDATE media SET collection_media_kind = ? WHERE id = ? AND kind = 'playlist'",
    )
    .bind(media_kind)
    .bind(playlist_id)
    .execute(db)
    .await
    .ok();
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaFilter {
    pub id: Option<Vec<Uuid>>,
    pub kind: Option<Vec<MediaKind>>,
    pub parent_id: Option<Uuid>,
    /// Filter by multiple parent IDs (OR). Used for programs by channel.
    pub parent_ids: Option<Vec<Uuid>>,
    pub promoted: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub recursive: bool,
    pub total_count: bool,
    pub include_user_state: bool,
    pub include_child_count: bool,
    pub include_relations: bool,
    /// User ID to use when loading user state (separate from user_state filter)
    pub user_id: Option<Uuid>,
    pub user_state: Option<super::UserMediaStateFilter>,
    pub genre_ids: Option<Vec<Uuid>>,
    pub studio_ids: Option<Vec<Uuid>>,
    pub person_ids: Option<Vec<Uuid>>,
    pub years: Option<Vec<i64>>,
    pub official_ratings: Option<Vec<String>>,
    pub max_parental_rating: Option<i32>,
    pub name_starts_with: Option<String>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_less_than: Option<String>,
    pub title_contains: Option<String>,
    pub index_number: Option<i64>,
    pub has_trailer: Option<bool>,
    /// GetItemsQuery.tags — item must have ANY of these tags
    pub tags: Option<Vec<String>>,
    /// From user policy — item must have NONE of these tags
    pub blocked_tags: Option<Vec<String>>,
    /// From user policy — if non-empty, item must have AT LEAST ONE of these tags
    pub allowed_tags: Option<Vec<String>>,
    /// Filter by enabled flag (for TvChannel). None = no filter.
    pub enabled: Option<bool>,
    /// If set, only return items whose parent has enabled = value (e.g. programs of enabled channels).
    pub parent_enabled: Option<bool>,
    /// Filter albums/tracks by artist (parent_id IN these IDs).
    pub artist_ids: Option<Vec<Uuid>>,
    /// If set, hides items whose digital release date exceeds this threshold.
    /// `digital_released_at` is used first. Items with no digital date but a `released_at`
    /// within the past year are always hidden (theatrical-only, digital date unknown).
    /// Older items without a digital date fall back to `released_at`.
    pub digital_released_before: Option<NaiveDateTime>,
    /// Sort order for results. Mapped from Jellyfin's ItemSortBy.
    pub sort_by: Vec<api::ItemSortBy>,
    pub sort_order: Vec<api::SortOrder>,
    /// For TvProgram queries: order by the parent channel's sort_order / channel_number.
    pub sort_by_channel_order: bool,
    /// Structured filter from a smart collection (groups of rules).
    pub filter_rules: Option<remux_sdks::remux::CollectionFilter>,
    /// Structured filter from user policy (applied separately, never on containers).
    pub policy_filter: Option<remux_sdks::remux::CollectionFilter>,
    /// Filter TvChannels by country code (ISO 3166-1 alpha-2, case-insensitive).
    pub country_filter: Option<String>,
    /// Filter TvChannels by group (M3U group-title / Xtream category).
    pub iptv_group_filter: Option<String>,
    /// For TvProgram: None = all, Some(true) = live_end < now, Some(false) = live_end >= now
    pub has_aired: Option<bool>,
    /// EPG window: live_end >= this value (program hasn't ended before window start)
    pub min_end_date: Option<NaiveDateTime>,
    /// EPG window: live_start <= this value (program starts before window end)
    pub max_start_date: Option<NaiveDateTime>,
    /// Filter TvPrograms by category (movie, series, news, kids, sports).
    pub program_kinds: Option<Vec<ProgramKind>>,
    /// Filter episodes/seasons/tracks by their grandparent (series, artist, etc.).
    pub grandparent_id: Option<Uuid>,
    /// Pre-fetched parent item. When set, `get_by_filter` uses it to detect
    /// manual collections and switches to a JOIN on media_relations.
    /// If `parent_id` is set but this is `None`, the non-JOIN path is used.
    pub parent: Option<Media>,
    /// Restrict Genre records to those related (via media_relations) to items
    /// of these content kinds. Used for smart-collection genre queries where
    /// items float freely and cannot be scoped via parent_id / CTE.
    pub genre_related_kinds: Option<Vec<MediaKind>>,
}

/// Normalise any country string to an ISO 3166-1 alpha-2 code (e.g. "US").
/// Accepts alpha-2 ("US"), alpha-3 ("USA"), or full English name ("United States of America").
/// Returns the input uppercased if no match is found.
pub fn normalize_country_alpha2(c: &str) -> String {
    let upper = c.to_uppercase();
    if upper.len() == 2 {
        return upper;
    }
    rust_iso3166::from_alpha3(&upper)
        .or_else(|| {
            rust_iso3166::ALL
                .iter()
                .find(|cc| {
                    cc.name
                        .eq_ignore_ascii_case(c)
                })
                .copied()
        })
        .map(|cc| {
            cc.alpha2
                .to_string()
        })
        .unwrap_or(upper)
}

/// Stream group filter/config data stored as JSON in the `stream_group_data` media column.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamGroupData {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub filter: remux_sdks::remux::StreamFilter,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct Media {
    // shared
    //#[sqlx(try_from="String")]
    #[default(get_uuid())]
    pub id: Uuid,
    pub title: String,
    #[default(MediaKind::Movie)]
    pub kind: MediaKind,
    #[default(chrono::Utc::now().naive_utc())]
    pub created_at: NaiveDateTime,
    #[default(chrono::Utc::now().naive_utc())]
    pub updated_at: NaiveDateTime,
    pub refreshed_at: Option<NaiveDateTime>,
    pub streams_refreshed_at: Option<NaiveDateTime>,

    // meta
    pub description: Option<String>,
    pub released_at: Option<NaiveDateTime>,
    pub digital_released_at: Option<NaiveDateTime>,
    #[sqlx(json(nullable))]
    pub trailers: Option<Vec<String>>,
    // in seconds
    pub runtime: Option<i64>,
    pub rating_critic: Option<f64>,
    pub rating_audience: Option<f64>,
    pub certification: Option<String>,
    #[sqlx(default)]
    pub certification_age: Option<i32>,
    /// ISO 3166-1 alpha-2 country code (e.g. "US", "GB").
    pub country: Option<String>,
    /// BCP 47 language tag of the original language (e.g. "en", "fr").
    pub original_language: Option<String>,
    #[sqlx(skip)]
    pub images: MediaImages,
    pub status: Option<MediaStatus>,
    pub idx: Option<i64>,
    pub parent_idx: Option<i64>,
    pub parent_id: Option<Uuid>,
    #[sqlx(default)]
    #[sqlx(json)]
    // NOTE: SQLx requires this to be valid JSON in the DB. Empty strings ('')
    // will cause decoding to fail with EOF. Use migration to fix existing rows.
    pub external_ids: ExternalIds,
    #[sqlx(json(nullable))]
    pub external_ratings: Option<ExternalRatings>,
    pub grandparent_id: Option<Uuid>,
    //pub season_id: Option<Uuid>,
    //pub description: Option<String>,
    #[sqlx(skip)]
    pub tags: Vec<String>,
    /// Set by TMDB meta fetch; written to `popularity_raw` by `save_pending_popularity`.
    #[sqlx(skip)]
    #[serde(skip)]
    pub pending_popularity: Option<(String, crate::addons::MetricValue)>,
    #[sqlx(skip)]
    pub child_count: Option<i64>,
    #[sqlx(skip)]
    pub recursive_item_count: Option<i64>,
    #[sqlx(skip)]
    pub album_count: Option<i64>,
    #[sqlx(skip)]
    pub song_count: Option<i64>,
    #[sqlx(skip)]
    pub movie_count: Option<i64>,
    #[sqlx(skip)]
    pub series_count: Option<i64>,
    #[sqlx(skip)]
    pub unplayed_item_count: Option<i64>,
    #[sqlx(skip)]
    pub sources: Option<Vec<Media>>,
    /// When this source represents a stream group in a filtered result,
    /// holds the group UUID to expose as the client-facing source ID.
    #[sqlx(skip)]
    #[serde(skip)]
    pub group_id: Option<Uuid>,
    #[sqlx(skip)]
    pub seasons: Option<Vec<Media>>,
    #[sqlx(skip)]
    pub episodes: Option<Vec<Media>>,
    #[sqlx(skip)]
    pub user_state: Option<super::UserMediaState>,
    #[sqlx(skip)]
    pub relations: Option<Vec<(MediaRelation, Media)>>,
    /// Preloaded direct parent (season, album, channel, etc.).
    #[sqlx(skip)]
    pub parent: Option<Box<Media>>,
    /// Preloaded grandparent (series, artist, etc.).
    #[sqlx(skip)]
    pub grandparent: Option<Box<Media>>,

    // stream
    #[sqlx(json(nullable))]
    pub stream_info: Option<crate::stream::StreamInfo>,
    #[sqlx(json(nullable))]
    pub probe_data: Option<MediaSourceInfo>,
    #[sqlx(json(nullable))]
    #[serde(skip)]
    pub stream_group_data: Option<StreamGroupData>,

    // collection
    pub promoted: bool,
    // CollectionKind
    pub collection_kind: Option<CollectionKind>,
    pub collection_latest_auto_unplayed: Option<bool>,
    pub collection_latest_sort_digital: Option<bool>,
    // CollectionMediaKind
    pub collection_media_kind: Option<CollectionMediaKind>,
    pub collection_max_items: Option<i64>,
    #[sqlx(json(nullable))]
    pub collection_smart_filter: Option<remux_sdks::remux::CollectionFilter>,
    /// For CollectionKind::Catalog: "addon_uuid:local_catalog_id" of the source catalog.
    pub collection_source: Option<String>,
    #[sqlx(json(nullable))]
    pub collection_default_sort: Option<Vec<sdks::remux::ItemSortBy>>,
    #[sqlx(json(nullable))]
    pub collection_default_sort_order: Option<Vec<sdks::remux::SortOrder>>,

    // IPTV / Live TV
    pub live_start: Option<NaiveDateTime>,
    pub live_end: Option<NaiveDateTime>,
    pub tvg_id: Option<String>,
    pub channel_number: Option<i64>,
    /// Whether this channel is shown to clients (true = enabled, false = hidden).
    #[default(true)]
    pub enabled: bool,
    /// User-defined display order for channels. Lower = earlier.
    pub sort_order: Option<i64>,
    /// User-defined name override; takes precedence over `title` for display.
    pub custom_name: Option<String>,
    pub program_kind: Option<ProgramKind>,
}

impl Media {
    /// Batch-load parent and grandparent `Media` records (with images) for tracks,
    /// albums, episodes, seasons, and TV programs, storing them as `self.parent` /
    /// `self.grandparent`. The API layer reads titles and image tags from those
    /// preloaded records instead of from flat denormalised fields.
    pub async fn preload_parents(db: &SqlitePool, records: &mut Vec<Self>) {
        let ids_needed: Vec<Uuid> = records
            .iter()
            .filter(|m| {
                matches!(
                    m.kind,
                    MediaKind::Track
                        | MediaKind::Album
                        | MediaKind::Episode
                        | MediaKind::Season
                        | MediaKind::TvProgram
                )
            })
            .flat_map(|m| {
                [m.parent_id, m.grandparent_id]
                    .into_iter()
                    .flatten()
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        if ids_needed.is_empty() {
            return;
        }

        // Lightweight fetch: only the columns the API layer needs from parent records.
        struct ParentRow {
            id: Uuid,
            title: String,
            channel_number: Option<i64>,
        }

        let mut parent_map: HashMap<Uuid, ParentRow> = HashMap::new();
        for chunk in ids_needed.chunks(500) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT id, title, channel_number FROM media WHERE id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            if let Ok(rows) = qb
                .build()
                .fetch_all(db)
                .await
            {
                parent_map.extend(
                    rows.into_iter()
                        .filter_map(|r| {
                            let id: Option<Uuid> = r.get(0);
                            let title: Option<String> = r.get(1);
                            let channel_number: Option<i64> = r.get(2);
                            id.zip(title)
                                .map(|(id, title)| {
                                    (
                                        id,
                                        ParentRow {
                                            id,
                                            title,
                                            channel_number,
                                        },
                                    )
                                })
                        }),
                );
            }
        }

        if parent_map.is_empty() {
            return;
        }

        let mut parent_images =
            super::image::MediaImage::get_for_media_ids(db, &ids_needed)
                .await
                .unwrap_or_default();

        // Build a synthetic Media stub from a ParentRow + its images.
        let make_stub =
            |row: &ParentRow, images: super::image::MediaImages| -> Box<Media> {
                let mut m = Media::default();
                m.id = row.id;
                m.title = row
                    .title
                    .clone();
                m.channel_number = row.channel_number;
                m.images = images;
                Box::new(m)
            };

        for media in records.iter_mut() {
            if !matches!(
                media.kind,
                MediaKind::Track
                    | MediaKind::Album
                    | MediaKind::Episode
                    | MediaKind::Season
                    | MediaKind::TvProgram
            ) {
                continue;
            }

            if let Some(pid) = media.parent_id {
                if let Some(row) = parent_map.get(&pid) {
                    let imgs = parent_images
                        .remove(&pid)
                        .unwrap_or_default();
                    media.parent = Some(make_stub(row, imgs));
                }
            }

            // For episodes grandparent_id points to the series;
            // fall back to parent_id for episodes with a flat hierarchy.
            let gp_id = match media.kind {
                MediaKind::Episode => media
                    .grandparent_id
                    .or(media.parent_id),
                _ => media.grandparent_id,
            };
            if let Some(gid) = gp_id {
                if let Some(row) = parent_map.get(&gid) {
                    let imgs = parent_images
                        .remove(&gid)
                        .unwrap_or_default();
                    media.grandparent = Some(make_stub(row, imgs));
                }
            }
        }
    }

    /// Build a minimal Media stub with just id and title — used when preloaded
    /// parent/grandparent data is constructed inline rather than fetched from DB.
    pub fn stub(id: Uuid, title: impl Into<String>) -> Box<Self> {
        let mut m = Self::default();
        m.id = id;
        m.title = title.into();
        Box::new(m)
    }

    pub fn parse_smart_filter(&self) -> Option<&remux_sdks::remux::CollectionFilter> {
        self.collection_smart_filter
            .as_ref()
    }

    pub fn is_remote_url(&self) -> bool {
        matches!(
            self.stream_info
                .as_ref()
                .map(|si| &si.descriptor),
            Some(crate::stream::StreamDescriptor::Http { .. })
        )
    }

    pub fn media_source_protocol(&self) -> &'static str {
        if self.is_remote_url() { "Http" } else { "File" }
    }
}

// #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
// pub struct SqlBool(pub bool);

// impl From<i32> for SqlBool {
//     fn from(value: i32) -> Self {
//         match value {
//             0 => Self(false),
//             1 => Self(true),
//             _ => panic!("invalid boolean value {value}"),
//         }
//     }
// }

#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Invalid media: {0}")]
    ValidationError(String),
}

impl Media {
    pub fn media_id_raw(&self) -> super::MediaIdRaw {
        super::MediaIdRaw {
            kind: self
                .kind
                .clone(),
            external_ids: self
                .external_ids
                .clone(),
            season: match self.kind {
                MediaKind::Season => self.idx,
                MediaKind::Episode => self.parent_idx,
                _ => None,
            },
            episode: if self.kind == MediaKind::Episode {
                self.idx
            } else {
                None
            },
        }
    }

    pub fn get_image(&self, kind: ImageKind) -> Option<&str> {
        self.images
            .get_path(kind)
    }

    pub fn set_image(&mut self, kind: ImageKind, url: String) {
        let media_id = self.id;
        let vec = match kind {
            ImageKind::Primary => {
                &mut self
                    .images
                    .primary
            }
            ImageKind::Backdrop => {
                &mut self
                    .images
                    .backdrop
            }
            ImageKind::Logo => {
                &mut self
                    .images
                    .logo
            }
            ImageKind::Thumb => {
                &mut self
                    .images
                    .thumb
            }
        };
        if let Some(existing) = vec
            .iter_mut()
            .find(|i| i.image_index == 0)
        {
            existing.path = url;
        } else {
            vec.push(MediaImage {
                id: Uuid::new_v4(),
                media_id,
                image_type: kind.to_string(),
                image_index: 0,
                path: url,
                width: None,
                height: None,
            });
        }
    }

    /// Whether the given user may delete media items.
    pub fn can_delete(user: &super::User) -> bool {
        user.is_admin
    }

    pub fn is_promoted(&self) -> bool {
        self.promoted
    }

    pub fn validate(&self) -> Result<(), MediaError> {
        if matches!(self.kind, MediaKind::Season | MediaKind::Episode)
            && self
                .idx
                .is_none()
        {
            return Err(MediaError::ValidationError(format!(
                "{:?} requires an index number",
                self.kind
            )));
        }

        let missing = match self.kind {
            MediaKind::Movie | MediaKind::Series => (self
                .external_ids
                .imdb
                .is_none()
                && self
                    .external_ids
                    .custom_stremio_id
                    .is_none())
            .then_some("imdb"),
            MediaKind::Season | MediaKind::Episode => (self
                .external_ids
                .series_imdb
                .is_none()
                && self
                    .external_ids
                    .series_custom_stremio_id
                    .is_none())
            .then_some("series_imdb"),
            MediaKind::Artist => self
                .external_ids
                .deezer_artist
                .is_none()
                .then_some("deezer_artist"),
            MediaKind::Album => (self
                .external_ids
                .deezer_album
                .is_none()
                && self
                    .external_ids
                    .youtube_id
                    .is_none())
            .then_some("deezer_album or youtube_id"),
            MediaKind::Track => (self
                .external_ids
                .deezer_track
                .is_none()
                && self
                    .external_ids
                    .youtube_id
                    .is_none())
            .then_some("deezer_track or youtube_id"),
            _ => None,
        };

        if let Some(field) = missing {
            return Err(MediaError::ValidationError(format!(
                "{:?} requires {field}",
                self.kind
            )));
        }

        // Verify the UUID is the stable deterministic value for this item's external IDs.
        // Random UUIDs break user state (favorites, continue watching) across purge+reimport.
        if matches!(
            self.kind,
            MediaKind::Movie
                | MediaKind::Series
                | MediaKind::Season
                | MediaKind::Episode
        ) {
            let expected = Uuid::from(&self.media_id_raw());
            if expected != self.id {
                return Err(MediaError::ValidationError(format!(
                    "{:?} '{}' UUID mismatch: id={} expected={}",
                    self.kind, self.title, self.id, expected
                )));
            }
        }

        if self.kind == MediaKind::Person {
            if let Some(tmdb_id) = self
                .external_ids
                .tmdb
            {
                let expected = crate::common::stable_media_uuid(
                    &MediaKind::Person,
                    &tmdb_id.to_string(),
                );
                if expected != self.id {
                    return Err(MediaError::ValidationError(format!(
                        "Person '{}' UUID mismatch: id={} expected={}",
                        self.title, self.id, expected
                    )));
                }
            }
        }

        Ok(())
    }

    pub async fn save(&mut self, db: &sqlx::SqlitePool) -> Result<()> {
        self.validate()?;
        let updated_at = Utc::now().naive_utc();

        sqlx::query(
        r#"
        INSERT INTO media (
            id, title, kind, parent_id, idx, released_at, runtime,
            rating_critic, rating_audience, description, trailers, stream_info, probe_data, promoted, collection_kind, collection_media_kind, collection_max_items,
            external_ids, external_ratings, created_at, updated_at, certification, certification_age, parent_idx,
            live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, refreshed_at, grandparent_id,
            collection_smart_filter, country, program_kind, collection_latest_auto_unplayed, collection_latest_sort_digital,
            collection_source, collection_default_sort, collection_default_sort_order,
            original_language
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41, $42, $43, $44)
        ON CONFLICT (id) DO UPDATE SET
            title = excluded.title,
            kind = excluded.kind,
            idx = COALESCE(excluded.idx, media.idx),
            released_at = COALESCE(excluded.released_at, media.released_at),
            digital_released_at = COALESCE(excluded.digital_released_at, media.digital_released_at),
            runtime = COALESCE(excluded.runtime, media.runtime),
            rating_critic = COALESCE(excluded.rating_critic, media.rating_critic),
            rating_audience = COALESCE(excluded.rating_audience, media.rating_audience),
            description = COALESCE(excluded.description, media.description),
            trailers = COALESCE(excluded.trailers, media.trailers),
            stream_info = COALESCE(excluded.stream_info, media.stream_info),
            probe_data = COALESCE(excluded.probe_data, media.probe_data),
            grandparent_id = excluded.grandparent_id,
            external_ids = excluded.external_ids,
            external_ratings = COALESCE(excluded.external_ratings, media.external_ratings),
            promoted = excluded.promoted,
            collection_kind = excluded.collection_kind,
            collection_media_kind = excluded.collection_media_kind,
            collection_max_items = excluded.collection_max_items,
            collection_smart_filter = excluded.collection_smart_filter,
            collection_latest_auto_unplayed = excluded.collection_latest_auto_unplayed,
            collection_latest_sort_digital = excluded.collection_latest_sort_digital,
            collection_source = excluded.collection_source,
            collection_default_sort = excluded.collection_default_sort,
            collection_default_sort_order = excluded.collection_default_sort_order,
            country = COALESCE(excluded.country, media.country),
            updated_at = excluded.updated_at,
            certification = excluded.certification,
            certification_age = excluded.certification_age,
            parent_idx = COALESCE(excluded.parent_idx, media.parent_idx),
            live_start = excluded.live_start,
            live_end = excluded.live_end,
            tvg_id = excluded.tvg_id,
            channel_number = excluded.channel_number,
            enabled = excluded.enabled,
            sort_order = excluded.sort_order,
            custom_name = excluded.custom_name,
            status = COALESCE(excluded.status, media.status),
            refreshed_at = COALESCE(excluded.refreshed_at, media.refreshed_at),
            program_kind = excluded.program_kind,
            original_language = COALESCE(excluded.original_language, media.original_language)
        "#,
        )
        .bind(self.id)
        .bind(&self.title)
        .bind(&self.kind)
        .bind(self.parent_id)
        .bind(self.idx)
        .bind(self.released_at)
        .bind(self.runtime)
        .bind(self.rating_critic)
        .bind(self.rating_audience)
        .bind(&self.description)
        .bind(sqlx::types::Json(&self.trailers))
        .bind(sqlx::types::Json(&self.stream_info))
        .bind(self.probe_data.as_ref().map(sqlx::types::Json))
        .bind(self.promoted)
        .bind(&self.collection_kind)
        .bind(&self.collection_media_kind)
        .bind(self.collection_max_items)
        .bind(sqlx::types::Json(&self.external_ids))
        .bind(sqlx::types::Json(&self.external_ratings))
        .bind(self.created_at)
        .bind(updated_at)
        .bind(&self.certification)
        .bind(self.certification_age)
        .bind(self.parent_idx)
        .bind(self.live_start)
        .bind(self.live_end)
        .bind(&self.tvg_id)
        .bind(self.channel_number)
        .bind(self.enabled)
        .bind(self.sort_order)
        .bind(&self.custom_name)
        .bind(self.digital_released_at)
        .bind(&self.status)
        .bind(self.refreshed_at)
        .bind(self.grandparent_id)
        .bind(sqlx::types::Json(&self.collection_smart_filter))
        .bind(self.country.as_deref().map(normalize_country_alpha2))
        .bind(&self.program_kind)
        .bind(self.collection_latest_auto_unplayed)
        .bind(self.collection_latest_sort_digital)
        .bind(&self.collection_source)
        .bind(sqlx::types::Json(&self.collection_default_sort))
        .bind(sqlx::types::Json(&self.collection_default_sort_order))
        .bind(&self.original_language)
        .execute(db)
        .await?;

        MediaImage::sync_from_media(db, self.id, &self.images)
            .await
            .ok();

        Ok(())
    }

    /// Invalidate the probe cache for a media source (e.g. after its URL changes).
    pub async fn clear_probe_data(db: &sqlx::SqlitePool, id: &Uuid) -> Result<()> {
        sqlx::query("UPDATE media SET probe_data = NULL WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn save_probe_data(
        db: &sqlx::SqlitePool,
        id: &Uuid,
        probe: &crate::api::MediaSourceInfo,
    ) -> Result<()> {
        sqlx::query("UPDATE media SET probe_data = ?1 WHERE id = ?2")
            .bind(sqlx::types::Json(probe))
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn insert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = db
            .begin()
            .await?;
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await?;

        for chunk in items.chunks(CHUNK_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, description, trailers, stream_info, probe_data, promoted, collection_kind, collection_media_kind,
                external_ids, external_ratings, created_at, updated_at, certification, certification_age, parent_idx,
                live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, grandparent_id, country, program_kind, collection_latest_auto_unplayed, collection_latest_sort_digital,
                collection_source, collection_default_sort, collection_default_sort_order,
                original_language
            )",
        );
            for item in chunk {
                item.validate()?;
            }
            query_builder.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.id)
                    .push_bind(&item.title)
                    .push_bind(&item.kind)
                    .push_bind(&item.parent_id)
                    .push_bind(&item.idx)
                    .push_bind(&item.released_at)
                    .push_bind(&item.runtime)
                    .push_bind(&item.rating_critic)
                    .push_bind(&item.rating_audience)
                    .push_bind(&item.description)
                    .push_bind(sqlx::types::Json(&item.trailers))
                    .push_bind(sqlx::types::Json(&item.stream_info))
                    .push_bind(
                        item.probe_data
                            .as_ref()
                            .map(sqlx::types::Json),
                    )
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(sqlx::types::Json(&item.external_ids))
                    .push_bind(sqlx::types::Json(&item.external_ratings))
                    .push_bind(&item.created_at)
                    .push_bind(Utc::now())
                    .push_bind(&item.certification)
                    .push_bind(&item.certification_age)
                    .push_bind(&item.parent_idx)
                    .push_bind(&item.live_start)
                    .push_bind(&item.live_end)
                    .push_bind(&item.tvg_id)
                    .push_bind(&item.channel_number)
                    .push_bind(&item.enabled)
                    .push_bind(&item.sort_order)
                    .push_bind(&item.custom_name)
                    .push_bind(&item.digital_released_at)
                    .push_bind(&item.status)
                    .push_bind(&item.grandparent_id)
                    .push_bind(
                        item.country
                            .as_deref()
                            .map(normalize_country_alpha2),
                    )
                    .push_bind(&item.program_kind)
                    .push_bind(&item.collection_latest_auto_unplayed)
                    .push_bind(&item.collection_latest_sort_digital)
                    .push_bind(&item.collection_source)
                    .push_bind(sqlx::types::Json(&item.collection_default_sort))
                    .push_bind(sqlx::types::Json(&item.collection_default_sort_order))
                    .push_bind(&item.original_language);
            });

            query_builder.push(" ON CONFLICT DO NOTHING");

            query_builder
                .build()
                .execute(&mut *tx)
                .await?;
        }

        tx.commit()
            .await?;
        Ok(())
    }

    pub async fn upsert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let items: Vec<Self> = items
            .iter()
            .filter(|item| match item.validate() {
                Ok(()) => true,
                Err(e) => {
                    error!(error = %e, "skipping media item with invalid UUID");
                    false
                }
            })
            .cloned()
            .collect();

        if items.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().naive_utc();

        for chunk in items.chunks(CHUNK_SIZE) {
            let _permit = DB_WRITE_SEMAPHORE
                .acquire()
                .await
                .unwrap();
            let mut tx = db
                .begin()
                .await?;
            sqlx::query("PRAGMA defer_foreign_keys = ON")
                .execute(&mut *tx)
                .await?;
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, description, trailers, stream_info, probe_data, promoted, collection_kind, collection_media_kind,
                external_ids, external_ratings, created_at, updated_at, certification, certification_age, parent_idx,
                live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, refreshed_at, grandparent_id, country, program_kind, collection_latest_auto_unplayed, collection_latest_sort_digital,
                collection_source, collection_default_sort, collection_default_sort_order,
                original_language
            )",
        );

            query_builder.push_values(chunk.iter(), |mut b, item| {
                b.push_bind(&item.id)
                    .push_bind(&item.title)
                    .push_bind(&item.kind)
                    .push_bind(&item.parent_id)
                    .push_bind(&item.idx)
                    .push_bind(&item.released_at)
                    .push_bind(&item.runtime)
                    .push_bind(&item.rating_critic)
                    .push_bind(&item.rating_audience)
                    .push_bind(&item.description)
                    .push_bind(sqlx::types::Json(&item.trailers))
                    .push_bind(sqlx::types::Json(&item.stream_info))
                    .push_bind(
                        item.probe_data
                            .as_ref()
                            .map(sqlx::types::Json),
                    )
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(sqlx::types::Json(&item.external_ids))
                    .push_bind(sqlx::types::Json(&item.external_ratings))
                    .push_bind(&item.created_at)
                    .push_bind(&now)
                    .push_bind(&item.certification)
                    .push_bind(&item.certification_age)
                    .push_bind(&item.parent_idx)
                    .push_bind(&item.live_start)
                    .push_bind(&item.live_end)
                    .push_bind(&item.tvg_id)
                    .push_bind(&item.channel_number)
                    .push_bind(&item.enabled)
                    .push_bind(&item.sort_order)
                    .push_bind(&item.custom_name)
                    .push_bind(&item.digital_released_at)
                    .push_bind(&item.status)
                    .push_bind(&item.refreshed_at)
                    .push_bind(&item.grandparent_id)
                    .push_bind(
                        item.country
                            .as_deref()
                            .map(normalize_country_alpha2),
                    )
                    .push_bind(&item.program_kind)
                    .push_bind(&item.collection_latest_auto_unplayed)
                    .push_bind(&item.collection_latest_sort_digital)
                    .push_bind(&item.collection_source)
                    .push_bind(sqlx::types::Json(&item.collection_default_sort))
                    .push_bind(sqlx::types::Json(&item.collection_default_sort_order))
                    .push_bind(&item.original_language);
            });

            query_builder.push(
                " ON CONFLICT DO UPDATE SET
                title = excluded.title,
                idx = COALESCE(excluded.idx, media.idx),
                released_at = COALESCE(excluded.released_at, media.released_at),
                digital_released_at = COALESCE(excluded.digital_released_at, media.digital_released_at),
                runtime = COALESCE(excluded.runtime, media.runtime),
                rating_critic = COALESCE(excluded.rating_critic, media.rating_critic),
                rating_audience = COALESCE(excluded.rating_audience, media.rating_audience),
                description = COALESCE(excluded.description, media.description),
                trailers = COALESCE(excluded.trailers, media.trailers),
                stream_info = COALESCE(excluded.stream_info, media.stream_info),
                external_ids = excluded.external_ids,
                external_ratings = COALESCE(excluded.external_ratings, media.external_ratings),
                probe_data = COALESCE(excluded.probe_data, media.probe_data),
                grandparent_id = excluded.grandparent_id,
                updated_at = excluded.updated_at,
                promoted = excluded.promoted,
                certification = excluded.certification,
                certification_age = excluded.certification_age,
                parent_id = excluded.parent_id,
                parent_idx = COALESCE(excluded.parent_idx, media.parent_idx),
                live_start = excluded.live_start,
                live_end = excluded.live_end,
                tvg_id = excluded.tvg_id,
                channel_number = excluded.channel_number,
                status = COALESCE(excluded.status, media.status),
                country = COALESCE(excluded.country, media.country),
                refreshed_at = COALESCE(excluded.refreshed_at, media.refreshed_at),
                -- preserve user overrides: only update name/enabled/sort_order if not set by user
                title = CASE WHEN custom_name IS NOT NULL THEN media.title ELSE excluded.title END,
                enabled = CASE WHEN media.id IS NOT NULL THEN media.enabled ELSE excluded.enabled END,
                sort_order = CASE WHEN media.id IS NOT NULL THEN media.sort_order ELSE excluded.sort_order END,
                custom_name = media.custom_name,
                program_kind = excluded.program_kind,
                original_language = COALESCE(excluded.original_language, media.original_language)",
            );

            query_builder
                .build()
                .execute(&mut *tx)
                .await?;

            let chunk_images: Vec<(Uuid, &MediaImage)> = chunk
                .iter()
                .flat_map(|m| {
                    m.images
                        .iter()
                        .map(move |img| (m.id, img))
                })
                .collect();
            for img_chunk in chunk_images.chunks(500) {
                let mut qb = sqlx::QueryBuilder::new(
                    "INSERT INTO media_images \
                     (id, media_id, image_type, image_index, path, width, height) ",
                );
                qb.push_values(img_chunk.iter(), |mut b, (media_id, img)| {
                    b.push_bind(Uuid::new_v4())
                        .push_bind(media_id)
                        .push_bind(&img.image_type)
                        .push_bind(img.image_index)
                        .push_bind(&img.path)
                        .push_bind(img.width)
                        .push_bind(img.height);
                });
                qb.push(
                    " ON CONFLICT (media_id, image_type, image_index) DO UPDATE SET \
                       id = excluded.id, path = excluded.path, \
                       width = excluded.width, height = excluded.height \
                     WHERE media_images.path LIKE 'http%' \
                       AND media_images.path <> excluded.path",
                );
                qb.build()
                    .execute(&mut *tx)
                    .await?;
            }

            tx.commit()
                .await?;
        }

        Ok(())
    }

    /// Return items of the same kind that share genres with `source_id`, scored by
    /// genre overlap count (descending).  Both `genre` and `music_genre` kinds are
    /// included.  Returns empty for episodes and items with no genres (matching
    /// Jellyfin behaviour).
    pub async fn get_similar_by_genres(
        db: &SqlitePool,
        source_id: &Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<(Uuid, i64)>, i64)> {
        // Get the source item's kind — only primary media types are supported.
        let kind_str: Option<String> =
            sqlx::query_scalar("SELECT kind FROM media WHERE id = ?")
                .bind(source_id)
                .fetch_optional(db)
                .await?;
        let Some(kind_str) = kind_str else {
            return Ok((vec![], 0));
        };
        let Ok(kind) = kind_str.parse::<MediaKind>() else {
            return Ok((vec![], 0));
        };
        if matches!(kind, MediaKind::Episode) {
            return Ok((vec![], 0));
        }

        // Collect genre IDs shared with the source item (both genre + music_genre).
        let genre_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT mr.right_media_id FROM media_relations mr \
             JOIN media g ON g.id = mr.right_media_id \
             WHERE mr.left_media_id = ? AND g.kind IN ('genre', 'music_genre')",
        )
        .bind(source_id)
        .fetch_all(db)
        .await?;
        if genre_ids.is_empty() {
            return Ok((vec![], 0));
        }

        // Build the similarity query using QueryBuilder throughout — never embed
        // raw `?` placeholders in the initial string, as push_bind appends its
        // own markers and the pre-baked ones would cause a syntax error.
        let base = "SELECT m.id, COUNT(DISTINCT mr.right_media_id) as score \
                    FROM media m \
                    JOIN media_relations mr ON mr.left_media_id = m.id \
                    JOIN media g ON g.id = mr.right_media_id \
                    WHERE m.kind = ";

        // Count total.
        let mut count_qb =
            sqlx::QueryBuilder::new(format!("SELECT COUNT(*) FROM ({} ", base));
        count_qb.push_bind(&kind_str);
        count_qb
            .push(" AND g.kind IN ('genre', 'music_genre') AND mr.right_media_id IN (");
        let mut sep = count_qb.separated(", ");
        for gid in &genre_ids {
            sep.push_bind(*gid);
        }
        count_qb.push(") AND m.id != ");
        count_qb.push_bind(source_id);
        count_qb.push(" GROUP BY m.id) sub");
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(db)
            .await?;

        // Fetch scored page.
        let mut qb = sqlx::QueryBuilder::new(base);
        qb.push_bind(&kind_str);
        qb.push(" AND g.kind IN ('genre', 'music_genre') AND mr.right_media_id IN (");
        let mut sep = qb.separated(", ");
        for gid in &genre_ids {
            sep.push_bind(*gid);
        }
        qb.push(") AND m.id != ");
        qb.push_bind(source_id);
        qb.push(" GROUP BY m.id ORDER BY score DESC LIMIT ");
        qb.push_bind(limit as i64);
        qb.push(" OFFSET ");
        qb.push_bind(offset as i64);

        let scored: Vec<(Uuid, i64)> = qb
            .build_query_as()
            .fetch_all(db)
            .await?;

        Ok((scored, total))
    }

    /// Return distinct Genre records linked (via media_relations) to media of the given kinds.
    /// If `related_kinds` is empty, all genres are returned.
    pub async fn get_genres(
        db: &SqlitePool,
        related_kinds: &[MediaKind],
    ) -> Result<Vec<Self>> {
        let mut qb = sqlx::QueryBuilder::new("SELECT DISTINCT g.* FROM media g");

        if !related_kinds.is_empty() {
            qb.push(" JOIN media_relations mr ON mr.right_media_id = g.id");
            qb.push(" JOIN media m ON mr.left_media_id = m.id");
            qb.push(" WHERE g.kind IN ('genre', 'music_genre') AND m.kind IN (");
            let mut sep = qb.separated(", ");
            for k in related_kinds {
                sep.push_bind(k);
            }
            qb.push(")");
        } else {
            qb.push(" WHERE g.kind IN ('genre', 'music_genre')");
        }

        qb.push(" ORDER BY g.title ASC");

        Ok(qb
            .build_query_as::<Self>()
            .fetch_all(db)
            .await?)
    }

    pub async fn get_by_id(
        db: &SqlitePool,
        id: &Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let mut row = sqlx::query_as::<_, Self>(
            r#"
        SELECT *
        FROM media
        WHERE id = $1
        "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        if let Some(ref mut media) = row {
            media.images = MediaImage::get_for_media(db, &media.id)
                .await
                .unwrap_or_default();
        }
        Ok(row)
    }

    pub async fn get_ancestors(db: &SqlitePool, id: &Uuid) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Self>(
            "WITH RECURSIVE ancestors AS (
                SELECT * FROM media WHERE id = (SELECT parent_id FROM media WHERE id = $1)
                UNION ALL
                SELECT m.* FROM media m JOIN ancestors a ON m.id = a.parent_id
            ) SELECT * FROM ancestors",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        Ok(rows)
    }

    pub async fn get_distinct_years(
        db: &SqlitePool,
        kinds: &[MediaKind],
    ) -> Result<Vec<i64>> {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT DISTINCT CAST(strftime('%Y', released_at) AS INTEGER) as y FROM media WHERE released_at IS NOT NULL",
        );
        if !kinds.is_empty() {
            qb.push(" AND kind IN (");
            let mut sep = qb.separated(", ");
            for k in kinds {
                sep.push_bind(k);
            }
            qb.push(")");
        }
        qb.push(" ORDER BY y DESC");
        let rows = qb
            .build()
            .fetch_all(db)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                use sqlx::Row;
                r.get::<Option<i64>, _>(0)
            })
            .collect())
    }

    pub async fn get_by_filter(
        db: &SqlitePool,
        filter: &MediaFilter,
    ) -> Result<FilterResult<Media>> {
        let is_manual_collection = filter
            .parent
            .as_ref()
            .map(|p| p.collection_kind == Some(CollectionKind::Manual))
            .unwrap_or(false);

        let is_smart_collection = filter
            .parent
            .as_ref()
            .map(|p| {
                matches!(
                    p.collection_kind,
                    Some(CollectionKind::Smart) | Some(CollectionKind::Catalog)
                )
            })
            .unwrap_or(false);

        let use_recursive = filter.recursive
            && filter
                .parent_id
                .is_some()
            && !is_manual_collection
            && !is_smart_collection;

        // Genres are flat global records linked to content via media_relations, not
        // via parent_id. When scoping a genre query to a parent collection/folder we
        // must filter by relation instead of by the normal parent_id/CTE scope.
        let is_genre_scope_query = filter
            .parent_id
            .is_some()
            && filter
                .kind
                .as_ref()
                .map(|k| {
                    !k.is_empty()
                        && k.iter()
                            .all(|k| {
                                matches!(k, MediaKind::Genre | MediaKind::MusicGenre)
                            })
                })
                .unwrap_or(false);

        // When sorting by DatePlayed, drive records_qb FROM user_media_state (dp) so
        // the result is already in last_played_at order — no correlated subquery per row,
        // no separate sort pass. Column names in subsequent WHERE clauses (kind, parent_id,
        // etc.) resolve unambiguously to media since dp only exposes (user_id, media_id,
        // last_played_at). Applied to all query shapes so dp.last_played_at in ORDER BY
        // is always valid when user_id is set.
        let date_played_uid = filter
            .sort_by
            .iter()
            .any(|s| matches!(s, api::ItemSortBy::DatePlayed))
            .then(|| {
                filter
                    .user_id
                    .as_ref()
            })
            .flatten();

        // When sorting by a single-period popularity metric, pre-compute scores via a
        // LEFT JOIN on a derived table so SQLite materialises popularity_agg once and
        // joins with a hash-join rather than executing 2 correlated subqueries per
        // qualifying row in ORDER BY. PopularityAllTime spans 3 periods and stays with
        // the correlated-subquery path.
        let pop_period: Option<&'static str> = filter
            .sort_by
            .iter()
            .find_map(|s| match s {
                api::ItemSortBy::TrendingWeek => Some("trend_week"),
                api::ItemSortBy::TrendingMonth => Some("trend_month"),
                api::ItemSortBy::PopularityDay => Some("daily"),
                api::ItemSortBy::PopularityWeek => Some("weekly"),
                api::ItemSortBy::PopularityMonth => Some("monthly"),
                _ => None,
            });
        let mut pop_joined = false;

        let mut count_qb;
        let mut records_qb;

        if use_recursive && !is_genre_scope_query {
            let parent_id = filter
                .parent_id
                .as_ref()
                .unwrap();

            count_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            count_qb.push_bind(parent_id);
            count_qb.push(
                " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                ) SELECT COUNT(*) as count FROM media WHERE id IN (SELECT id FROM subtree) AND 1=1",
            );

            records_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            records_qb.push_bind(parent_id);
            if let Some(uid) = date_played_uid {
                // CROSS JOIN prevents SQLite from reordering the tables, forcing
                // user_media_state as the outer loop. Combined with
                // idx_ums_user_last_played(user_id, last_played_at DESC), SQLite
                // scans the user's plays in order and can stop at LIMIT without
                // sorting the full result set. The join condition is in WHERE so
                // the planner still applies it as a filter (not a cartesian product).
                records_qb.push(
                    " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                    ) SELECT media.* FROM user_media_state dp CROSS JOIN media \
                    WHERE dp.user_id = ",
                );
                records_qb.push_bind(uid);
                records_qb.push(" AND dp.media_id = media.id AND media.id IN (SELECT id FROM subtree) AND 1=1");
            } else {
                records_qb.push(
                    " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                    ) SELECT * FROM media WHERE id IN (SELECT id FROM subtree) AND 1=1",
                );
            }
        } else if use_recursive && is_genre_scope_query {
            // CTE at top level so we can reference it in the relation subquery below,
            // but the base query is plain — no id IN subtree baked in.
            let parent_id = filter
                .parent_id
                .as_ref()
                .unwrap();

            count_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            count_qb.push_bind(parent_id);
            count_qb.push(
                " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                ) SELECT COUNT(*) as count FROM media WHERE 1=1",
            );

            records_qb = sqlx::QueryBuilder::new(
                "WITH RECURSIVE subtree AS (SELECT id FROM media WHERE parent_id = ",
            );
            records_qb.push_bind(parent_id);
            if let Some(uid) = date_played_uid {
                records_qb.push(
                    " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                    ) SELECT media.* FROM user_media_state dp CROSS JOIN media \
                    WHERE dp.user_id = ",
                );
                records_qb.push_bind(uid);
                records_qb.push(" AND dp.media_id = media.id AND 1=1");
            } else {
                records_qb.push(
                    " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                    ) SELECT * FROM media WHERE 1=1",
                );
            }
        } else if is_manual_collection {
            let collection_id = filter
                .parent_id
                .as_ref()
                .unwrap();

            count_qb = sqlx::QueryBuilder::new(
                "SELECT COUNT(*) as count FROM media \
                 JOIN media_relations mr ON mr.right_media_id = media.id \
                 AND mr.role = 'collection' AND mr.left_media_id = ",
            );
            count_qb.push_bind(collection_id);
            count_qb.push(" WHERE 1=1");

            if let Some(uid) = date_played_uid {
                records_qb = sqlx::QueryBuilder::new(
                    "SELECT media.* FROM user_media_state dp CROSS JOIN media \
                     JOIN media_relations mr ON mr.right_media_id = media.id \
                     AND mr.role = 'collection' AND mr.left_media_id = ",
                );
                records_qb.push_bind(collection_id);
                records_qb.push(" WHERE dp.user_id = ");
                records_qb.push_bind(uid);
                records_qb.push(" AND dp.media_id = media.id AND 1=1");
            } else {
                records_qb = sqlx::QueryBuilder::new(
                    "SELECT media.* FROM media \
                     JOIN media_relations mr ON mr.right_media_id = media.id \
                     AND mr.role = 'collection' AND mr.left_media_id = ",
                );
                records_qb.push_bind(collection_id);
                records_qb.push(" WHERE 1=1");
            }
        } else {
            count_qb = sqlx::QueryBuilder::new(
                "SELECT COUNT(*) as count FROM media WHERE 1=1",
            );
            if let Some(uid) = date_played_uid {
                records_qb = sqlx::QueryBuilder::new(
                    "SELECT media.* FROM user_media_state dp \
                     CROSS JOIN media WHERE dp.user_id = ",
                );
                records_qb.push_bind(uid);
                records_qb.push(" AND media.id = dp.media_id AND 1=1");
            } else if let Some(period) = pop_period {
                // Materialise the latest per-media popularity score once and JOIN it in
                // so ORDER BY uses a plain column reference instead of N correlated
                // subqueries — one per qualifying row before LIMIT is applied.
                pop_joined = true;
                records_qb = sqlx::QueryBuilder::new(format!(
                    "SELECT media.* FROM media \
                     LEFT JOIN popularity_agg pop \
                       ON pop.media_id = media.id \
                      AND pop.period = '{period}' \
                      AND pop.latest = 1 \
                     WHERE 1=1"
                ));
            } else {
                records_qb = sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");
            }
        }

        // Pre-fetch in-progress media IDs — JOIN media so kind and date filters are applied
        // here rather than in the main query. The main query then contains only
        // `WHERE media.id IN (ids)` which forces SQLite to use individual PK lookups
        // (O(n_ids)) instead of scanning the entire kind-filtered media table (O(total_media)).
        let resumable_ids: Option<Vec<uuid::Uuid>> =
            if let Some(usf) = &filter.user_state {
                if usf.resumable == Some(true) {
                    let ids: Vec<uuid::Uuid> = if let Some(user_id) = &usf.user_id {
                        // Drive from user_media_state (small, indexed by user_id) and
                        // check media conditions via a correlated EXISTS so SQLite does
                        // one PK lookup per in-progress item instead of materialising
                        // the entire kind/date-filtered media set.
                        let mut pre_qb = sqlx::QueryBuilder::new(
                            "SELECT media_id FROM user_media_state \
                         WHERE user_id = ",
                        );
                        pre_qb.push_bind(user_id);
                        pre_qb.push(" AND playback_position > 0 AND play_count = 0");
                        let needs_media_filter = filter
                            .kind
                            .as_ref()
                            .map(|k| !k.is_empty())
                            .unwrap_or(false)
                            || filter
                                .digital_released_before
                                .is_some();
                        if needs_media_filter {
                            pre_qb.push(
                                " AND EXISTS (SELECT 1 FROM media \
                             WHERE id = media_id AND 1=1",
                            );
                            if let Some(kinds) = &filter.kind {
                                if !kinds.is_empty() {
                                    pre_qb.push(" AND kind IN (");
                                    let mut sep = pre_qb.separated(", ");
                                    for k in kinds {
                                        sep.push_bind(k);
                                    }
                                    pre_qb.push(")");
                                }
                            }
                            if let Some(&threshold) = filter
                                .digital_released_before
                                .as_ref()
                            {
                                push_release_date_filter(
                                    &mut pre_qb,
                                    "media",
                                    threshold,
                                    true,
                                );
                            }
                            pre_qb.push(")");
                        }
                        pre_qb
                            .build_query_scalar::<uuid::Uuid>()
                            .fetch_all(db)
                            .await?
                    } else {
                        vec![]
                    };
                    Some(ids)
                } else {
                    None
                }
            } else {
                None
            };

        // series_excluded: no series possible → use NOT IN bloom filter for unplayed
        // series_only:     only series → emit episode EXISTS directly (no CASE wrapper)
        // else (mixed):    OR-split so non-series still get the bloom filter
        let series_excluded = filter
            .kind
            .as_ref()
            .map(|k| !k.is_empty() && !k.contains(&MediaKind::Series))
            .unwrap_or(false);
        let series_only = filter
            .kind
            .as_ref()
            .map(|k| {
                !k.is_empty()
                    && k.iter()
                        .all(|k| matches!(k, MediaKind::Series))
            })
            .unwrap_or(false);

        for qb in [&mut count_qb, &mut records_qb] {
            if is_genre_scope_query {
                // Filter genres by their media_relations to items within the parent scope.
                if use_recursive {
                    qb.push(
                        " AND id IN (\
                            SELECT DISTINCT mr.right_media_id FROM media_relations mr \
                            WHERE mr.left_media_id IN (SELECT id FROM subtree)\
                        )",
                    );
                } else if is_manual_collection {
                    let cid = filter
                        .parent_id
                        .as_ref()
                        .unwrap();
                    qb.push(
                        " AND id IN (\
                            SELECT DISTINCT mr.right_media_id FROM media_relations mr \
                            WHERE mr.left_media_id IN (\
                                SELECT right_media_id FROM media_relations \
                                WHERE left_media_id = ",
                    );
                    qb.push_bind(cid);
                    qb.push(" AND role = 'collection'))");
                }
            } else if !use_recursive && !is_manual_collection && !is_smart_collection {
                if let Some(parent_id) = &filter.parent_id {
                    qb.push(" AND parent_id = ")
                        .push_bind(parent_id);
                }
                if let Some(parent_ids) = &filter.parent_ids {
                    if !parent_ids.is_empty() {
                        qb.push(" AND parent_id IN (");
                        let mut sep = qb.separated(", ");
                        for id in parent_ids {
                            sep.push_bind(id);
                        }
                        qb.push(")");
                    }
                }
            }
            if let Some(related_kinds) = &filter.genre_related_kinds {
                if !related_kinds.is_empty() {
                    qb.push(
                        " AND id IN (\
                            SELECT DISTINCT mr.right_media_id FROM media_relations mr \
                            JOIN media item ON item.id = mr.left_media_id \
                            WHERE item.kind IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for k in related_kinds {
                        sep.push_bind(k);
                    }
                    qb.push("))");
                }
            }
            if let Some(grandparent_id) = &filter.grandparent_id {
                qb.push(" AND grandparent_id = ")
                    .push_bind(grandparent_id);
            }
            if let Some(promoted) = &filter.promoted {
                qb.push(" AND promoted = ")
                    .push_bind(promoted);
            }
            if let Some(kind) = &filter.kind {
                if resumable_ids.is_none() {
                    qb.push_in("kind", &kind);
                }
            }
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }

            if let Some(genre_ids) = &filter.genre_ids {
                if !genre_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in genre_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(artist_ids) = &filter.artist_ids {
                if !artist_ids.is_empty() {
                    qb.push(" AND (parent_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in artist_ids {
                        sep.push_bind(id);
                    }
                    qb.push(") OR grandparent_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in artist_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(user_state_filter) = &filter.user_state {
                // favorite — always uses EXISTS
                if let Some(favorite) = &user_state_filter.favorite {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_id = media.id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ")
                            .push_bind(user_id);
                    }
                    qb.push(" AND ums.favorite = ")
                        .push_bind(favorite)
                        .push(")");
                }

                // played=true — EXISTS with play_count > 0
                if user_state_filter.played == Some(true) {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_id = media.id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ")
                            .push_bind(user_id);
                    }
                    qb.push(" AND ums.play_count > 0)");
                }

                // played=false (unplayed).
                // reconcile_series_played_state keeps the series' own play_count
                // in sync with its episodes, so a simple NOT EXISTS on the row
                // itself works for both movies and series — no need to traverse
                // the episode tree.
                if user_state_filter.played == Some(false) {
                    qb.push(
                        " AND NOT EXISTS (SELECT 1 FROM user_media_state ums \
                                          WHERE ums.media_id = media.id",
                    );
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ")
                            .push_bind(user_id.clone());
                    }
                    qb.push(" AND ums.play_count > 0)");
                }

                // resumable — IDs pre-fetched above; bind directly so SQLite uses PK
                // lookups instead of scanning all kind-matching media rows.
                if user_state_filter.resumable == Some(true) {
                    if let Some(ref ids) = resumable_ids {
                        if ids.is_empty() {
                            qb.push(" AND 1=0");
                        } else {
                            qb.push(" AND media.id IN (");
                            let mut sep = qb.separated(", ");
                            for id in ids {
                                sep.push_bind(*id);
                            }
                            qb.push(")");
                        }
                    }
                }
            }

            if let Some(years) = &filter.years {
                if !years.is_empty() {
                    qb.push(" AND CAST(strftime('%Y', released_at) AS INTEGER) IN (");
                    let mut sep = qb.separated(", ");
                    for y in years {
                        sep.push_bind(y);
                    }
                    qb.push(")");
                }
            }

            if let Some(ratings) = &filter.official_ratings {
                if !ratings.is_empty() {
                    qb.push(" AND certification IN (");
                    let mut sep = qb.separated(", ");
                    for r in ratings {
                        sep.push_bind(r);
                    }
                    qb.push(")");
                }
            }

            if let Some(max_rating) = filter.max_parental_rating {
                qb.push(" AND (certification_age IS NULL OR certification_age <= ")
                    .push_bind(max_rating)
                    .push(")");
            }

            if let Some(s) = &filter.name_starts_with {
                // LIKE is case-insensitive for ASCII in SQLite; no UPPER() needed.
                // A COLLATE NOCASE index on title can satisfy this as a prefix scan.
                qb.push(" AND title LIKE ")
                    .push_bind(format!("{}%", s));
            }

            if let Some(s) = &filter.name_starts_with_or_greater {
                qb.push(" AND title >= ")
                    .push_bind(s.clone())
                    .push(" COLLATE NOCASE");
            }

            if let Some(s) = &filter.name_less_than {
                qb.push(" AND title < ")
                    .push_bind(s.clone())
                    .push(" COLLATE NOCASE");
            }

            if let Some(s) = &filter.title_contains {
                qb.push(" AND title LIKE ")
                    .push_bind(format!("%{}%", s));
            }

            if let Some(idx) = &filter.index_number {
                qb.push(" AND idx = ")
                    .push_bind(idx);
            }

            if let Some(true) = &filter.has_trailer {
                qb.push(" AND json_array_length(trailers) > 0");
            }
            if let Some(false) = &filter.has_trailer {
                qb.push(" AND (trailers IS NULL OR json_array_length(trailers) = 0)");
            }

            if let Some(studio_ids) = &filter.studio_ids {
                if !studio_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in studio_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            if let Some(person_ids) = &filter.person_ids {
                if !person_ids.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in person_ids {
                        sep.push_bind(id);
                    }
                    qb.push("))");
                }
            }

            // GetItemsQuery.tags: item must have ANY of these tags
            if let Some(tags) = &filter.tags {
                if !tags.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_tags mt WHERE mt.media_id = media.id AND mt.tag IN (");
                    let mut sep = qb.separated(", ");
                    for t in tags {
                        sep.push_bind(t);
                    }
                    qb.push("))");
                }
            }

            // User policy blocked_tags: hide if item has ANY blocked tag
            if let Some(blocked) = &filter.blocked_tags {
                if !blocked.is_empty() {
                    qb.push(" AND NOT EXISTS (SELECT 1 FROM media_tags mt WHERE mt.media_id = media.id AND mt.tag IN (");
                    let mut sep = qb.separated(", ");
                    for t in blocked {
                        sep.push_bind(t);
                    }
                    qb.push("))");
                }
            }

            // User policy allowed_tags: only show if item has AT LEAST ONE allowed tag
            if let Some(allowed) = &filter.allowed_tags {
                if !allowed.is_empty() {
                    qb.push(" AND EXISTS (SELECT 1 FROM media_tags mt WHERE mt.media_id = media.id AND mt.tag IN (");
                    let mut sep = qb.separated(", ");
                    for t in allowed {
                        sep.push_bind(t);
                    }
                    qb.push("))");
                }
            }

            if let Some(enabled) = &filter.enabled {
                qb.push(" AND enabled = ")
                    .push_bind(*enabled);
            }

            if let Some(c) = &filter.country_filter {
                qb.push(" AND country = ")
                    .push_bind(c.to_uppercase());
            }

            if let Some(g) = &filter.iptv_group_filter {
                qb.push(" AND json_extract(external_ids, '$.iptv_group') = ")
                    .push_bind(g);
            }

            if let Some(parent_enabled) = &filter.parent_enabled {
                qb.push(" AND parent_id IN (SELECT id FROM media WHERE kind = 'tv_channel' AND enabled = ")
                    .push_bind(*parent_enabled)
                    .push(")");
            }

            if let Some(has_aired) = filter.has_aired {
                if has_aired {
                    qb.push(" AND live_end < datetime('now')");
                } else {
                    qb.push(" AND live_end >= datetime('now')");
                }
            }

            if let Some(min_end) = &filter.min_end_date {
                qb.push(" AND live_end >= ")
                    .push_bind(min_end);
            }

            if let Some(max_start) = &filter.max_start_date {
                qb.push(" AND live_start <= ")
                    .push_bind(max_start);
            }

            if let Some(kinds) = &filter.program_kinds {
                if !kinds.is_empty() {
                    qb.push_in("program_kind", kinds);
                }
            }

            if let Some(&threshold) = filter
                .digital_released_before
                .as_ref()
            {
                if resumable_ids.is_none() {
                    let season_only = filter
                        .kind
                        .as_ref()
                        .map(|k| {
                            !k.is_empty()
                                && k.iter()
                                    .all(|k| matches!(k, MediaKind::Season))
                        })
                        .unwrap_or(false);
                    push_release_date_filter(qb, "media", threshold, !season_only);
                }
            }

            if let Some(ref f) = filter.filter_rules {
                apply_filter_rules(qb, f);
            }
            if let Some(ref f) = filter.policy_filter {
                apply_filter_rules(qb, f);
            }
        }
        // Apply ORDER BY driven by the sort_by field, with per-kind fallbacks.
        let is_channel_query = filter
            .kind
            .as_ref()
            .map(|k| {
                k.iter()
                    .all(|k| matches!(k, MediaKind::TvChannel))
            })
            .unwrap_or(false);

        if !filter
            .sort_by
            .is_empty()
        {
            let mut order_clauses: Vec<String> = filter
                .sort_by
                .iter()
                .enumerate()
                .map(|(i, sort)| {
                    let order = filter
                        .sort_order
                        .get(i)
                        .or_else(|| filter.sort_order.first())
                        .copied()
                        .unwrap_or(api::SortOrder::Ascending);
                    let dir = match order {
                        api::SortOrder::Ascending => "ASC",
                        api::SortOrder::Descending => "DESC",
                    };
                    let col = match sort {
                        api::ItemSortBy::SortName | api::ItemSortBy::Name => {
                            format!("title COLLATE NOCASE {}", dir)
                        }
                        api::ItemSortBy::DateCreated => {
                            format!("datetime(created_at) {}", dir)
                        }
                        api::ItemSortBy::PremiereDate
                        | api::ItemSortBy::ProductionYear => {
                            format!(
                                "COALESCE(released_at, digital_released_at) {}",
                                dir
                            )
                        }
                        api::ItemSortBy::DigitalReleaseDate => {
                            format!("COALESCE(digital_released_at, released_at) {}", dir)
                        }
                        api::ItemSortBy::CommunityRating => {
                            format!("COALESCE(rating_audience, rating_critic) {}", dir)
                        }
                        api::ItemSortBy::IndexNumber => {
                            format!("COALESCE(idx, 999999) {}", dir)
                        }
                        api::ItemSortBy::ParentIndexNumber => {
                            format!("COALESCE(parent_idx, 999999) {}", dir)
                        }
                        api::ItemSortBy::Runtime => {
                            format!("COALESCE(runtime, 0) {}", dir)
                        }
                        api::ItemSortBy::DatePlayed => {
                            if filter.user_id.is_some() {
                                // dp alias from the UMS-driven records_qb above.
                                format!("dp.last_played_at {}", dir)
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::Random => "RANDOM()".to_string(),
                        api::ItemSortBy::ChannelOrder => {
                            format!("(sort_order IS NULL), COALESCE(sort_order, channel_number, 999999) {dir}, title COLLATE NOCASE")
                        }
                        api::ItemSortBy::CatalogOrder => {
                            let catalog_ids: Vec<String> = filter
                                .filter_rules
                                .iter()
                                .flat_map(|cf| cf.groups.iter().flat_map(|g| g.rules.iter()))
                                .find_map(|r| {
                                    if let sdks::remux::FilterRule::Catalog { catalog_ids, .. } = r {
                                        Some(catalog_ids.iter().map(|id| id.simple().to_string()).collect())
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or_default();
                            if !catalog_ids.is_empty() {
                                let in_clause = catalog_ids
                                    .iter()
                                    .map(|hex| format!("X'{hex}'"))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                format!(
                                    "COALESCE((SELECT MIN(mr.weight) FROM media_relations mr \
                                     WHERE mr.right_media_id = media.id AND mr.role = 'catalog' \
                                     AND mr.left_media_id IN ({in_clause})), 999999) ASC"
                                )
                            } else {
                                format!("title COLLATE NOCASE {dir}")
                            }
                        }
                        api::ItemSortBy::PopularityAllTime => {
                            // all-time → most recent yearly → most recent monthly → 0
                            "COALESCE(\
                               (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'all' AND pa.period_key = 'all'),\
                               (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'yearly' ORDER BY pa.period_key DESC LIMIT 1),\
                               (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'monthly' ORDER BY pa.period_key DESC LIMIT 1),\
                               0) DESC"
                                .to_string()
                        }
                        api::ItemSortBy::PopularityDay => {
                            if pop_joined {
                                "pop.avg DESC NULLS LAST".to_string()
                            } else {
                                "COALESCE(\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'daily' AND pa.period_key = date('now')),\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'daily' ORDER BY pa.period_key DESC LIMIT 1),\
                                   0) DESC"
                                    .to_string()
                            }
                        }
                        api::ItemSortBy::PopularityWeek => {
                            if pop_joined {
                                "pop.avg DESC NULLS LAST".to_string()
                            } else {
                                "COALESCE(\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'weekly' AND pa.period_key = date('now', 'weekday 0', '-6 days')),\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'weekly' ORDER BY pa.period_key DESC LIMIT 1),\
                                   0) DESC"
                                    .to_string()
                            }
                        }
                        api::ItemSortBy::PopularityMonth => {
                            if pop_joined {
                                "pop.avg DESC NULLS LAST".to_string()
                            } else {
                                "COALESCE(\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'monthly' AND pa.period_key = strftime('%Y-%m', 'now')),\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'monthly' ORDER BY pa.period_key DESC LIMIT 1),\
                                   0) DESC"
                                    .to_string()
                            }
                        }
                        api::ItemSortBy::TrendingWeek => {
                            if pop_joined {
                                "pop.avg DESC NULLS LAST".to_string()
                            } else {
                                "COALESCE(\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'trend_week' AND pa.period_key = date('now')),\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'trend_week' ORDER BY pa.period_key DESC LIMIT 1),\
                                   0) DESC"
                                    .to_string()
                            }
                        }
                        api::ItemSortBy::TrendingMonth => {
                            if pop_joined {
                                "pop.avg DESC NULLS LAST".to_string()
                            } else {
                                "COALESCE(\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'trend_month' AND pa.period_key = date('now')),\
                                   (SELECT pa.avg FROM popularity_agg pa WHERE pa.media_id = media.id AND pa.period = 'trend_month' ORDER BY pa.period_key DESC LIMIT 1),\
                                   0) DESC"
                                    .to_string()
                            }
                        }
                        // Default fallback
                        _ => format!("title COLLATE NOCASE {}", dir),
                    };
                    col
                })
                .collect();
            records_qb.push(" ORDER BY ");
            records_qb.push(order_clauses.join(", "));
        } else if is_manual_collection {
            records_qb.push(" ORDER BY mr.weight ASC");
        } else if filter.sort_by_channel_order {
            records_qb.push(
                " ORDER BY (SELECT COALESCE(c.sort_order, c.channel_number, 999999) FROM media c WHERE c.id = media.parent_id)",
            );
        } else if is_channel_query {
            records_qb.push(
                " ORDER BY (sort_order IS NULL), COALESCE(sort_order, channel_number, 999999), title COLLATE NOCASE",
            );
        }

        if let Some(limit) = &filter.limit {
            records_qb
                .push(" LIMIT ")
                .push_bind(limit);
        } else if filter
            .offset
            .is_some()
        {
            records_qb.push(" LIMIT -1");
        }
        if let Some(offset) = &filter.offset {
            records_qb
                .push(" OFFSET ")
                .push_bind(offset);
        }

        let (count, records_result) = tokio::join!(
            async {
                if !filter.total_count {
                    return Ok(0_usize);
                }
                let query = count_qb.build();
                let row = query
                    .fetch_one(db)
                    .await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<Media>();
                query
                    .fetch_all(db)
                    .await
            }
        );
        let mut records = records_result?;
        if !records.is_empty() {
            let ids: Vec<Uuid> = records
                .iter()
                .map(|m| m.id)
                .collect();
            let mut tags_qb = sqlx::QueryBuilder::new(
                "SELECT media_id, tag FROM media_tags WHERE media_id IN (",
            );
            let mut sep = tags_qb.separated(", ");
            for id in &ids {
                sep.push_bind(id);
            }
            tags_qb.push(") ORDER BY tag");
            let tag_rows = tags_qb
                .build()
                .fetch_all(db)
                .await?;
            let mut tags_map: HashMap<Uuid, Vec<String>> = HashMap::new();
            for row in tag_rows {
                let media_id: Uuid = row.get(0);
                let tag: String = row.get(1);
                tags_map
                    .entry(media_id)
                    .or_default()
                    .push(tag);
            }
            for media in &mut records {
                if let Some(tags) = tags_map.remove(&media.id) {
                    media.tags = tags;
                }
            }

            let mut images_map = MediaImage::get_for_media_ids(db, &ids)
                .await
                .unwrap_or_default();
            for media in &mut records {
                media.images = images_map
                    .remove(&media.id)
                    .unwrap_or_default();
            }
        }

        let rel_ids: Vec<Uuid> = if filter.include_relations {
            records
                .iter()
                .filter(|m| {
                    matches!(
                        m.kind,
                        MediaKind::Movie
                            | MediaKind::Episode
                            | MediaKind::Series
                            | MediaKind::Season
                    )
                })
                .map(|m| m.id)
                .collect()
        } else {
            vec![]
        };
        if !rel_ids.is_empty() {
            let mut g_qb = sqlx::QueryBuilder::new(
                // Drive from media_relations using the left_media_id index.
                // Filtering g.kind in SQL caused the planner to drive from the
                // media table (scanning all persons/genres) instead — very slow.
                // We filter by kind in Rust after the fetch.
                "SELECT mr.left_media_id, mr.relation_id, mr.right_media_id, mr.weight, \
                 mr.role, mr.character, g.id, g.title, g.kind \
                 FROM media_relations mr \
                 JOIN media g ON g.id = mr.right_media_id \
                 WHERE mr.left_media_id IN (",
            );
            let mut sep = g_qb.separated(", ");
            for id in &rel_ids {
                sep.push_bind(id);
            }
            g_qb.push(") ORDER BY mr.left_media_id, mr.weight");
            match g_qb
                .build()
                .fetch_all(db)
                .await
            {
                Ok(rows) => {
                    let mut rels_map: HashMap<Uuid, Vec<(MediaRelation, Media)>> =
                        HashMap::new();
                    for row in rows {
                        let kind_str: String = row.get(8);
                        let Ok(kind) = MediaKind::try_from(kind_str) else {
                            continue;
                        };
                        if !matches!(
                            kind,
                            MediaKind::Genre
                                | MediaKind::MusicGenre
                                | MediaKind::Person
                                | MediaKind::Studio
                                | MediaKind::Country
                        ) {
                            continue;
                        }
                        let left_media_id: Uuid = row.get(0);
                        let rel = MediaRelation {
                            relation_id: row.get(1),
                            left_media_id,
                            right_media_id: row.get(2),
                            weight: row.get(3),
                            role: row.get(4),
                            character: row.get(5),
                            ..Default::default()
                        };
                        let related = Media {
                            id: row.get(6),
                            title: row.get(7),
                            kind,
                            ..Default::default()
                        };
                        rels_map
                            .entry(left_media_id)
                            .or_default()
                            .push((rel, related));
                    }
                    let related_ids: Vec<Uuid> = rels_map
                        .values()
                        .flat_map(|v| {
                            v.iter()
                                .map(|(_, m)| m.id)
                        })
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect();
                    let mut related_images =
                        MediaImage::get_for_media_ids(db, &related_ids)
                            .await
                            .unwrap_or_default();
                    for rels in rels_map.values_mut() {
                        for (_, m) in rels.iter_mut() {
                            if let Some(imgs) = related_images.remove(&m.id) {
                                m.images = imgs;
                            }
                        }
                    }
                    for media in &mut records {
                        if let Some(rels) = rels_map.remove(&media.id) {
                            media.relations = Some(rels);
                        }
                    }
                }
                Err(e) => {
                    warn!("failed to batch-load relations: {e}");
                }
            }
        }

        if filter.include_child_count && !records.is_empty() {
            let folder_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| {
                    matches!(
                        m.kind,
                        MediaKind::Series
                            | MediaKind::Season
                            | MediaKind::Collection
                            | MediaKind::Folder
                            | MediaKind::Album
                            | MediaKind::Artist
                    )
                })
                .map(|m| m.id)
                .collect();
            if !folder_ids.is_empty() {
                let mut cc_qb = sqlx::QueryBuilder::new(
                    "SELECT parent_id, COUNT(*) as cnt FROM media WHERE parent_id IN (",
                );
                let mut sep = cc_qb.separated(", ");
                for id in &folder_ids {
                    sep.push_bind(id);
                }
                cc_qb.push(") GROUP BY parent_id");
                match cc_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    Ok(cc_rows) => {
                        let mut cc_map: HashMap<Uuid, i64> = HashMap::new();
                        for row in cc_rows {
                            let pid: Uuid = row.get(0);
                            let cnt: i64 = row.get(1);
                            cc_map.insert(pid, cnt);
                        }
                        for media in &mut records {
                            if let Some(&cnt) = cc_map.get(&media.id) {
                                media.child_count = Some(cnt);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("failed to load child counts: {e}");
                    }
                }
            }

            // For playlists: count items via media_relations
            let playlist_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| m.kind == MediaKind::Playlist)
                .map(|m| m.id)
                .collect();
            if !playlist_ids.is_empty() {
                let mut pl_qb = sqlx::QueryBuilder::new(
                    "SELECT left_media_id, COUNT(*) FROM media_relations WHERE role = 'playlist' AND left_media_id IN (",
                );
                let mut sep = pl_qb.separated(", ");
                for id in &playlist_ids {
                    sep.push_bind(id);
                }
                pl_qb.push(") GROUP BY left_media_id");
                match pl_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    Ok(rows) => {
                        let mut cc_map: HashMap<Uuid, i64> = HashMap::new();
                        for row in rows {
                            let pid: Uuid = row.get(0);
                            let cnt: i64 = row.get(1);
                            cc_map.insert(pid, cnt);
                        }
                        for media in &mut records {
                            if media.kind == MediaKind::Playlist {
                                media.child_count = Some(
                                    *cc_map
                                        .get(&media.id)
                                        .unwrap_or(&0),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!("failed to load playlist child counts: {e}");
                    }
                }
            }

            // For series: populate recursive_item_count with total episode count
            let series_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| m.kind == MediaKind::Series)
                .map(|m| m.id)
                .collect();
            if !series_ids.is_empty() {
                let mut ep_qb = sqlx::QueryBuilder::new(
                    "SELECT grandparent_id, COUNT(*) as cnt FROM media WHERE kind = 'episode' AND grandparent_id IN (",
                );
                let mut sep = ep_qb.separated(", ");
                for id in &series_ids {
                    sep.push_bind(id);
                }
                ep_qb.push(") GROUP BY grandparent_id");
                if let Ok(rows) = ep_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Series {
                            media.recursive_item_count = map
                                .get(&media.id)
                                .copied();
                        }
                    }
                }
            }

            // For persons: count movies and series they appear in
            let person_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| m.kind == MediaKind::Person)
                .map(|m| m.id)
                .collect();
            if !person_ids.is_empty() {
                // movie_count
                let mut movie_qb = sqlx::QueryBuilder::new(
                    "SELECT mr.right_media_id, COUNT(DISTINCT mr.left_media_id) \
                     FROM media_relations mr \
                     JOIN media m ON m.id = mr.left_media_id AND m.kind = 'movie' \
                     WHERE mr.right_media_id IN (",
                );
                let mut sep = movie_qb.separated(", ");
                for id in &person_ids {
                    sep.push_bind(id);
                }
                movie_qb.push(") GROUP BY mr.right_media_id");
                if let Ok(rows) = movie_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Person {
                            media.movie_count = map
                                .get(&media.id)
                                .copied();
                        }
                    }
                }

                // series_count
                let mut series_qb = sqlx::QueryBuilder::new(
                    "SELECT mr.right_media_id, COUNT(DISTINCT mr.left_media_id) \
                     FROM media_relations mr \
                     JOIN media m ON m.id = mr.left_media_id AND m.kind = 'series' \
                     WHERE mr.right_media_id IN (",
                );
                let mut sep = series_qb.separated(", ");
                for id in &person_ids {
                    sep.push_bind(id);
                }
                series_qb.push(") GROUP BY mr.right_media_id");
                if let Ok(rows) = series_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Person {
                            media.series_count = map
                                .get(&media.id)
                                .copied();
                        }
                    }
                }

                // child_count = movie_count + series_count
                for media in &mut records {
                    if media.kind == MediaKind::Person {
                        media.child_count = Some(
                            media
                                .movie_count
                                .unwrap_or(0)
                                + media
                                    .series_count
                                    .unwrap_or(0),
                        );
                    }
                }
            }

            // For artists: populate album_count and song_count
            let artist_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| m.kind == MediaKind::Artist)
                .map(|m| m.id)
                .collect();
            if !artist_ids.is_empty() {
                let mut alb_qb = sqlx::QueryBuilder::new(
                    "SELECT parent_id, COUNT(*) as cnt FROM media WHERE kind = 'album' AND parent_id IN (",
                );
                let mut sep = alb_qb.separated(", ");
                for id in &artist_ids {
                    sep.push_bind(id);
                }
                alb_qb.push(") GROUP BY parent_id");
                if let Ok(rows) = alb_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Artist {
                            media.album_count = map
                                .get(&media.id)
                                .copied();
                        }
                    }
                }

                let mut song_qb = sqlx::QueryBuilder::new(
                    "SELECT grandparent_id, COUNT(*) as cnt FROM media WHERE kind = 'track' AND grandparent_id IN (",
                );
                let mut sep = song_qb.separated(", ");
                for id in &artist_ids {
                    sep.push_bind(id);
                }
                song_qb.push(") GROUP BY grandparent_id");
                if let Ok(rows) = song_qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Artist {
                            media.song_count = map
                                .get(&media.id)
                                .copied();
                        }
                    }
                }
            }
        }

        Self::preload_parents(db, &mut records).await;

        if filter.include_user_state {
            let uid = filter
                .user_id
                .or_else(|| {
                    filter
                        .user_state
                        .as_ref()
                        .and_then(|s| s.user_id)
                });
            if let Some(user_id) = uid {
                let media_ids: Vec<Uuid> = records
                    .iter()
                    .map(|m| m.id)
                    .collect();

                let states = super::UserMediaState::get_by_filter(
                    db,
                    &super::UserMediaStateFilter {
                        user_id: Some(user_id),
                        media_id: Some(media_ids),
                        ..Default::default()
                    },
                )
                .await?
                .records;

                let states_map: HashMap<Uuid, super::UserMediaState> = states
                    .into_iter()
                    .map(|state| (state.media_id, state))
                    .collect();

                for media in &mut records {
                    if let Some(state) = states_map.get(&media.id) {
                        media.user_state = Some(state.clone());
                    }
                }

                // Compute unplayed episode count for series/seasons
                let grandparent_ids: Vec<Uuid> = records
                    .iter()
                    .filter(|m| matches!(m.kind, MediaKind::Series | MediaKind::Season))
                    .map(|m| m.id)
                    .collect();

                if !grandparent_ids.is_empty() {
                    // Count episodes per grandparent_id that have NOT been played by this user
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT e.grandparent_id, COUNT(*) as cnt FROM media e \
                         WHERE e.kind = 'episode' AND e.grandparent_id IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for id in &grandparent_ids {
                        sep.push_bind(id);
                    }
                    qb.push(
                        ") AND NOT EXISTS (\
                           SELECT 1 FROM user_media_state ums \
                           WHERE ums.media_id = e.id \
                           AND ums.user_id = ",
                    );
                    qb.push_bind(user_id);
                    qb.push(" AND ums.play_count > 0)");
                    if let Some(t) = filter.digital_released_before {
                        push_release_date_filter(&mut qb, "e", t, true);
                    }
                    qb.push(" GROUP BY e.grandparent_id");

                    match qb
                        .build()
                        .fetch_all(db)
                        .await
                    {
                        Ok(rows) => {
                            let mut unplayed_map: HashMap<Uuid, i64> = HashMap::new();
                            for row in rows {
                                let sid: Uuid = row.get(0);
                                let cnt: i64 = row.get(1);
                                unplayed_map.insert(sid, cnt);
                            }
                            for media in &mut records {
                                if matches!(
                                    media.kind,
                                    MediaKind::Series | MediaKind::Season
                                ) {
                                    media.unplayed_item_count = Some(
                                        unplayed_map
                                            .get(&media.id)
                                            .copied()
                                            .unwrap_or(0),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("failed to load unplayed counts: {e}");
                        }
                    }
                }
            }
        }

        Ok(FilterResult {
            records,
            total_count: count?,
        })
    }

    pub async fn get_refreshable(
        db: &SqlitePool,
        limit: u32,
        offset: u32,
        total_count: bool,
    ) -> Result<(Vec<Self>, Option<u32>)> {
        const WHERE: &str = r#"
        WHERE kind IN (?, ?)
          AND (
            refreshed_at IS NULL
            OR (kind = 'series' AND (status IS NULL OR status != 'ended') AND datetime(created_at) < datetime('now', '-1 hour'))
            OR (digital_released_at IS NULL AND datetime(created_at) < datetime('now', '-1 hour'))
          )"#;

        let total = if total_count {
            let row: (i64,) =
                sqlx::query_as(&format!("SELECT COUNT(*) FROM media {WHERE}"))
                    .bind(MediaKind::Movie)
                    .bind(MediaKind::Series)
                    .fetch_one(db)
                    .await?;
            Some(row.0 as u32)
        } else {
            None
        };

        let rows = sqlx::query_as::<_, Self>(&format!(
            "SELECT * FROM media {WHERE} ORDER BY id LIMIT ? OFFSET ?"
        ))
        .bind(MediaKind::Movie)
        .bind(MediaKind::Series)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;

        Ok((rows, total))
    }

    pub async fn get_by_jellyfin_filter(
        db: &sqlx::SqlitePool,
        filter: &api::GetItemsQuery,
        total_count: bool,
        user: Option<&super::User>,
        server_config: Option<&api::ServerConfiguration>,
        smart_filter: Option<&remux_sdks::remux::CollectionFilter>,
        parent: Option<&Media>,
    ) -> Result<FilterResult<Media>> {
        let user_policy = user
            .and_then(|u| {
                u.policy
                    .as_ref()
            })
            .map(|p| &p.0);
        // Map media_types (Video, Book, ...) to MediaKind constraints
        let media_type_kinds: Option<Vec<MediaKind>> = filter
            .media_types
            .as_ref()
            .map(|types| {
                types
                    .iter()
                    .flat_map(|t| match t {
                        api::MediaType::Video => {
                            vec![MediaKind::Movie, MediaKind::Episode]
                        }
                        api::MediaType::Audio => vec![MediaKind::Track],
                        _ => vec![],
                    })
                    .collect()
            });

        // media_types was specified but maps to no kinds we serve — return empty
        if matches!(&media_type_kinds, Some(v) if v.is_empty()) {
            return Ok(FilterResult {
                records: vec![],
                total_count: 0,
            });
        }

        let kinds = if let Some(include_item_types) = &filter.include_item_types {
            let mut ikt_kinds: Vec<MediaKind> = include_item_types
                .iter()
                .filter_map(|t| MediaKind::try_from(t.clone()).ok())
                .collect();
            // Genre and MusicGenre are two sides of the same concept; always expand.
            if ikt_kinds.contains(&MediaKind::Genre)
                && !ikt_kinds.contains(&MediaKind::MusicGenre)
            {
                ikt_kinds.push(MediaKind::MusicGenre);
            }
            // If types were specified but none map to a known kind (e.g. MusicVideo),
            // return empty rather than falling through to an unbounded query.
            if ikt_kinds.is_empty() {
                return Ok(FilterResult {
                    records: vec![],
                    total_count: 0,
                });
            }
            if let Some(mt_kinds) = media_type_kinds {
                // Container types (Playlist, Collection, etc.) are not content — don't gate
                // them by mediaTypes, which describes playable content like Audio/Video.
                let intersection: Vec<MediaKind> = ikt_kinds
                    .into_iter()
                    .filter(|k| {
                        matches!(
                            k,
                            MediaKind::Playlist
                                | MediaKind::Collection
                                | MediaKind::Folder
                        ) || mt_kinds.contains(k)
                    })
                    .collect();
                if intersection.is_empty() {
                    return Ok(FilterResult {
                        records: vec![],
                        total_count: 0,
                    });
                }
                intersection
            } else {
                ikt_kinds
            }
        } else if let Some(mt_kinds) = media_type_kinds {
            mt_kinds
        } else {
            Vec::new()
        };

        // Resolve genre names → IDs
        let genre_ids_from_names: Option<Vec<Uuid>> =
            if let Some(names) = &filter.genres {
                if names.is_empty() {
                    None
                } else {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT id FROM media WHERE kind = 'genre' AND title IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for n in names {
                        sep.push_bind(n);
                    }
                    qb.push(")");
                    let rows = qb
                        .build()
                        .fetch_all(db)
                        .await?;
                    Some(
                        rows.into_iter()
                            .filter_map(|r| r.get::<Option<Uuid>, _>(0))
                            .collect(),
                    )
                }
            } else {
                None
            };

        // Resolve studio names → IDs
        let studio_ids_from_names: Option<Vec<Uuid>> =
            if let Some(names) = &filter.studios {
                if names.is_empty() {
                    None
                } else {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT id FROM media WHERE kind = 'studio' AND title IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for n in names {
                        sep.push_bind(n);
                    }
                    qb.push(")");
                    let rows = qb
                        .build()
                        .fetch_all(db)
                        .await?;
                    Some(
                        rows.into_iter()
                            .filter_map(|r| r.get::<Option<Uuid>, _>(0))
                            .collect(),
                    )
                }
            } else {
                None
            };

        // Merge genre IDs from query param and from genre names
        let genre_ids: Option<Vec<Uuid>> = {
            let from_param: Option<Vec<Uuid>> = filter
                .genre_ids
                .as_ref()
                .map(|ids| {
                    ids.iter()
                        .flat_map(|s| s.split(','))
                        .filter_map(|s| {
                            s.trim()
                                .parse::<Uuid>()
                                .ok()
                        })
                        .collect()
                });
            match (from_param, genre_ids_from_names) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        };

        // Merge studio IDs from query param and from studio names
        let studio_ids: Option<Vec<Uuid>> = {
            let from_param: Option<Vec<Uuid>> = filter
                .studio_ids
                .as_ref()
                .map(|ids| {
                    ids.iter()
                        .flat_map(|s| s.split(','))
                        .filter_map(|s| {
                            s.trim()
                                .parse::<Uuid>()
                                .ok()
                        })
                        .collect()
                });
            match (from_param, studio_ids_from_names) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        };

        let person_ids: Option<Vec<Uuid>> = filter
            .person_ids
            .as_ref()
            .map(|ids| {
                ids.iter()
                    .flat_map(|s| s.split(','))
                    .filter_map(|s| {
                        s.trim()
                            .parse::<Uuid>()
                            .ok()
                    })
                    .collect()
            });

        // Build user-state filter from is_favorite + filters[] items
        let item_filters = filter
            .filters
            .as_deref()
            .unwrap_or(&[]);
        let is_played = item_filters.contains(&api::ItemFilter::IsPlayed);
        let is_unplayed = item_filters.contains(&api::ItemFilter::IsUnplayed);
        let is_resumable = item_filters.contains(&api::ItemFilter::IsResumable);
        let favorite = filter
            .is_favorite
            .or_else(|| {
                item_filters
                    .contains(&api::ItemFilter::IsFavorite)
                    .then_some(true)
            });

        let user_state =
            if favorite.is_some() || is_played || is_unplayed || is_resumable {
                Some(super::UserMediaStateFilter {
                    user_id: filter.user_id,
                    favorite,
                    played: if is_played {
                        Some(true)
                    } else if is_unplayed {
                        Some(false)
                    } else {
                        None
                    },
                    resumable: if is_resumable { Some(true) } else { None },
                    ..Default::default()
                })
            } else {
                None
            };

        let has_tv_channel = kinds.contains(&MediaKind::TvChannel);
        let has_playlist = kinds.contains(&MediaKind::Playlist);
        // True only when the query exclusively targets container kinds (no content mixed in).
        // Used to skip content filter rules on container queries and to hide empty containers.
        let targeting_containers = !kinds.is_empty()
            && kinds
                .iter()
                .all(|k| matches!(k, MediaKind::Collection | MediaKind::Folder));

        let release_date_applies = !kinds.is_empty()
            && kinds
                .iter()
                .any(|k| {
                    matches!(
                        k,
                        MediaKind::Movie
                            | MediaKind::Series
                            | MediaKind::Season
                            | MediaKind::Episode
                    )
                });
        let digital_released_before = release_date_applies
            .then(|| server_config.and_then(|c| c.release_date_threshold()))
            .flatten();

        let user_policy_filter = user_policy.and_then(|p| {
            p.filter_rules
                .as_ref()
        });

        let mut result = Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(kinds),
                enabled: has_tv_channel.then_some(true),
                promoted: filter.promoted,
                limit: filter
                    .limit
                    .clone(),
                id: filter
                    .ids
                    .clone(),
                // album_ids maps directly to parent_id (tracks are children of albums)
                parent_id: filter
                    .parent_id
                    .clone()
                    .or_else(|| {
                        filter
                            .album_ids
                            .as_ref()
                            .and_then(|v| {
                                v.first()
                                    .cloned()
                            })
                    }),
                offset: filter
                    .start_index
                    .clone(),
                recursive: filter.recursive,
                include_user_state: filter
                    .enable_user_data
                    .unwrap_or(true),
                user_id: filter.user_id,
                include_child_count: has_playlist
                    || filter
                        .fields
                        .as_deref()
                        .map(|f| f.contains(&api::ItemFields::ChildCount))
                        .unwrap_or(false),
                include_relations: filter
                    .fields
                    .as_deref()
                    .map(|f| {
                        f.contains(&api::ItemFields::People)
                            || f.contains(&api::ItemFields::Genres)
                            || f.contains(&api::ItemFields::Studios)
                            || f.contains(&api::ItemFields::ProductionLocations)
                    })
                    .unwrap_or(false),
                total_count,
                user_state,
                genre_ids,
                studio_ids,
                person_ids,
                years: filter
                    .years
                    .clone(),
                official_ratings: filter
                    .official_ratings
                    .clone(),
                max_parental_rating: user_policy.and_then(|p| p.max_parental_rating),
                name_starts_with: filter
                    .name_starts_with
                    .clone(),
                name_starts_with_or_greater: filter
                    .name_starts_with_or_greater
                    .clone(),
                name_less_than: filter
                    .name_less_than
                    .clone(),
                title_contains: filter
                    .search_term
                    .clone(),
                index_number: filter.index_number,
                has_trailer: filter.has_trailer,
                tags: filter
                    .tags
                    .clone(),
                blocked_tags: user_policy
                    .map(|p| {
                        p.blocked_tags
                            .clone()
                    })
                    .filter(|v| !v.is_empty()),
                allowed_tags: user_policy
                    .map(|p| {
                        p.allowed_tags
                            .clone()
                    })
                    .filter(|v| !v.is_empty()),
                digital_released_before,
                sort_by: filter
                    .sort_by
                    .clone()
                    .unwrap_or_default(),
                sort_order: filter
                    .sort_order
                    .clone()
                    .unwrap_or_default(),
                filter_rules: smart_filter.cloned(),
                // Content filter rules must not apply to container queries — only
                // to content (movies, episodes, etc.). See CLAUDE.md.
                policy_filter: if !targeting_containers {
                    user_policy_filter.cloned()
                } else {
                    None
                },
                artist_ids: filter
                    .artist_ids
                    .clone()
                    .or_else(|| {
                        filter
                            .contributing_artist_ids
                            .clone()
                    })
                    .or_else(|| {
                        filter
                            .album_artist_ids
                            .clone()
                    }),
                grandparent_id: filter.series_id,
                parent: parent.cloned(),
                ..Default::default()
            },
        )
        .await?;

        // Hide containers that contain zero items visible to the user after applying
        // their content filter rules.
        if targeting_containers
            && !result
                .records
                .is_empty()
        {
            if let Some(pf) = user_policy_filter {
                if pf
                    .groups
                    .iter()
                    .any(|g| {
                        !g.rules
                            .is_empty()
                    })
                {
                    let container_ids: Vec<uuid::Uuid> = result
                        .records
                        .iter()
                        .map(|m| m.id)
                        .collect();
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT parent_id, COUNT(*) FROM media WHERE parent_id IN (",
                    );
                    let mut sep = qb.separated(", ");
                    for id in &container_ids {
                        sep.push_bind(*id);
                    }
                    qb.push(
                        ") AND kind NOT IN ('collection', 'folder', 'playlist', 'tv_channel')",
                    );
                    apply_filter_rules(&mut qb, pf);
                    qb.push(" GROUP BY parent_id");

                    if let Ok(rows) = qb
                        .build()
                        .fetch_all(db)
                        .await
                    {
                        let counts: HashMap<uuid::Uuid, i64> = rows
                            .into_iter()
                            .map(|r| (r.get::<uuid::Uuid, _>(0), r.get::<i64, _>(1)))
                            .collect();
                        result
                            .records
                            .retain(|m| {
                                counts
                                    .get(&m.id)
                                    .copied()
                                    .unwrap_or(0)
                                    > 0
                            });
                        result.total_count = result
                            .records
                            .len();
                    }
                }
            }
        }

        Ok(result)
    }

    pub async fn into_base_item(
        self,
        db: &sqlx::SqlitePool,
    ) -> Result<api::BaseItemDto> {
        //  let provider_ids = ProviderIds::get_by_media_id(db, &self.id).await?;

        let mut item = api::BaseItemDto {
            id: self.id,
            server_id: server_id(),
            type_: self
                .kind
                .clone()
                .into(),
            parent_id: self.parent_id,
            index_number: self.idx,
            name: Some(match self.kind {
                MediaKind::Episode => format!(
                    "Episode {}",
                    self.idx
                        .unwrap_or(0)
                ),
                MediaKind::Season => format!(
                    "Season {}",
                    self.idx
                        .unwrap_or(0)
                ),
                _ => self
                    .title
                    .clone(),
            }),
            is_folder: matches!(self.kind, MediaKind::Series | MediaKind::Season),
            ..Default::default()
        };

        Ok(item)
    }

    pub async fn delete(db: &SqlitePool, id: &Uuid) -> Result<()> {
        sqlx::query("DELETE FROM media WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub async fn parent(&self, db: &sqlx::SqlitePool) -> Result<Option<Self>> {
        if let Some(parent_id) = &self.parent_id {
            Ok(Self::get_by_id(db, parent_id).await?)
        } else {
            Ok(None)
        }
    }

    /// Set only this item's played state — no propagation.
    async fn apply_played(
        &self,
        db: &SqlitePool,
        user: &super::User,
        now: chrono::NaiveDateTime,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = state
            .play_count
            .max(1);
        state.played_at = Some(now);
        state
            .save(db)
            .await?;
        Ok(state)
    }

    /// Clear only this item's played state — no propagation.
    async fn apply_unplayed(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = 0;
        state.played_at = None;
        state.playback_position = 0;
        state
            .save(db)
            .await?;
        Ok(state)
    }

    pub async fn mark_played(
        &self,
        db: &SqlitePool,
        user: &super::User,
        recursive: bool,
        release_threshold: Option<chrono::NaiveDateTime>,
    ) -> Result<super::UserMediaState> {
        let now = Local::now().naive_local();
        let state = self
            .apply_played(db, user, now)
            .await?;

        if !recursive {
            return Ok(state);
        }

        match self.kind {
            MediaKind::Episode => {
                if let Some(season_id) = self.parent_id {
                    let unplayed = count_unplayed_children(
                        db,
                        season_id,
                        MediaKind::Episode,
                        user.id,
                        release_threshold,
                    )
                    .await;
                    if unplayed == 0 {
                        if let Ok(Some(season)) = Self::get_by_id(db, &season_id).await
                        {
                            season
                                .apply_played(db, user, now)
                                .await?;
                            cascade_played_to_series(
                                db,
                                user,
                                &season,
                                now,
                                release_threshold,
                            )
                            .await?;
                        }
                    }
                }
            }

            MediaKind::Season => {
                let episode_ids =
                    child_episode_ids(db, self.id, release_threshold).await;
                bulk_mark_played(db, user.id, &episode_ids, now).await;
                cascade_played_to_series(db, user, self, now, release_threshold)
                    .await?;
            }

            MediaKind::Series => {
                let season_ids = child_season_ids(db, self.id, release_threshold).await;
                bulk_mark_played(db, user.id, &season_ids, now).await;
                let episode_ids =
                    grandchild_episode_ids(db, self.id, release_threshold).await;
                bulk_mark_played(db, user.id, &episode_ids, now).await;
            }

            _ => {}
        }

        Ok(state)
    }

    pub async fn mark_unplayed(
        &self,
        db: &SqlitePool,
        user: &super::User,
        recursive: bool,
    ) -> Result<super::UserMediaState> {
        let state = self
            .apply_unplayed(db, user)
            .await?;

        if !recursive {
            return Ok(state);
        }

        match self.kind {
            MediaKind::Episode => {
                unplay_parent_if_played(db, user, self.parent_id).await?;
                unplay_parent_if_played(db, user, self.grandparent_id).await?;
            }

            MediaKind::Season => {
                let episode_ids = child_episode_ids(db, self.id, None).await;
                bulk_mark_unplayed(db, user.id, &episode_ids).await;
                unplay_parent_if_played(db, user, self.parent_id).await?;
            }

            MediaKind::Series => {
                let season_ids = child_season_ids(db, self.id, None).await;
                bulk_mark_unplayed(db, user.id, &season_ids).await;
                let episode_ids = grandchild_episode_ids(db, self.id, None).await;
                bulk_mark_unplayed(db, user.id, &episode_ids).await;
            }

            _ => {}
        }

        Ok(state)
    }

    pub async fn mark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = true;
        state
            .save(db)
            .await?;
        Ok(state)
    }

    pub async fn unmark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = false;
        state
            .save(db)
            .await?;
        Ok(state)
    }

    pub async fn streams(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self
            .sources
            .is_none()
        {
            let mut sources = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Stream]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            sources.sort_by(|a, b| {
                a.idx
                    .cmp(&b.idx)
            });

            // Exclude Sources that predate the last refresh — they belong to a
            // previous fetch and may have expired URLs. They stay in the DB so
            // an ongoing playback session can still reach them by direct ID.
            if let Some(refreshed) = self.streams_refreshed_at {
                sources.retain(|s| s.updated_at >= refreshed);
            }

            self.sources = Some(sources);
        };
        Ok(self
            .sources
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }

    pub async fn seasons(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Series {
            return Ok(vec![]);
        }

        if self
            .seasons
            .is_none()
        {
            let seasons = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Season]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            self.seasons = Some(seasons);
        }

        Ok(self
            .seasons
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }

    pub async fn episodes(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Season {
            return Ok(vec![]);
        }

        if self
            .episodes
            .is_none()
        {
            let episodes = Self::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Episode]),
                    parent_id: Some(self.id),
                    ..Default::default()
                },
            )
            .await?
            .records;

            self.episodes = Some(episodes);
        }

        Ok(self
            .episodes
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }

    pub async fn user_state(
        &mut self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<Option<super::UserMediaState>> {
        if self
            .user_state
            .is_none()
        {
            let state = super::UserMediaState::get_or_new(db, user, self).await?;

            self.user_state = Some(state);
        }

        Ok(self
            .user_state
            .clone())
    }

    pub async fn load_relations(&mut self, db: &SqlitePool) -> Result<()> {
        if self
            .relations
            .is_some()
        {
            return Ok(());
        }

        let rels = MediaRelation::get_by_media_id(db, &self.id).await?;
        if rels.is_empty() {
            self.relations = Some(vec![]);
            return Ok(());
        }

        let media_ids: Vec<Uuid> = rels
            .iter()
            .map(|r| r.right_media_id)
            .collect();
        let related = Self::get_by_filter(
            db,
            &MediaFilter {
                id: Some(media_ids),
                ..Default::default()
            },
        )
        .await?
        .records;

        let map: std::collections::HashMap<Uuid, Media> = related
            .into_iter()
            .map(|m| (m.id, m))
            .collect();

        let pairs = rels
            .into_iter()
            .filter_map(|rel| {
                map.get(&rel.right_media_id)
                    .map(|m| (rel, m.clone()))
            })
            .collect();

        self.relations = Some(pairs);
        Ok(())
    }

    /// Count items by kind
    pub async fn count_by_kind(db: &SqlitePool, kind: &MediaKind) -> Result<i64> {
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media WHERE kind = ?1")
                .bind(kind)
                .fetch_one(db)
                .await?;
        Ok(count)
    }
}

async fn count_unplayed_children(
    db: &SqlitePool,
    parent_id: Uuid,
    kind: MediaKind,
    user_id: Uuid,
    threshold: Option<chrono::NaiveDateTime>,
) -> i64 {
    let mut qb =
        sqlx::QueryBuilder::new("SELECT COUNT(*) FROM media WHERE parent_id = ");
    qb.push_bind(parent_id);
    qb.push(" AND kind = ");
    qb.push_bind(kind.to_string());
    qb.push(
        " AND NOT EXISTS (\
           SELECT 1 FROM user_media_state ums \
           WHERE ums.media_id = media.id \
           AND ums.user_id = ",
    );
    qb.push_bind(user_id);
    qb.push(" AND ums.play_count > 0)");
    if let Some(t) = threshold {
        push_release_date_filter(&mut qb, "media", t, true);
    }
    qb.build_query_scalar()
        .fetch_one(db)
        .await
        .unwrap_or(1)
}

async fn cascade_played_to_series(
    db: &SqlitePool,
    user: &super::User,
    season: &Media,
    now: chrono::NaiveDateTime,
    release_threshold: Option<chrono::NaiveDateTime>,
) -> anyhow::Result<()> {
    if let Some(series_id) = season.parent_id {
        let unplayed =
            count_unplayed_released_seasons(db, series_id, user.id, release_threshold)
                .await;
        if unplayed == 0 {
            if let Ok(Some(series)) = Media::get_by_id(db, &series_id).await {
                series
                    .apply_played(db, user, now)
                    .await?;
            }
        }
    }
    Ok(())
}

/// Count seasons under `series_id` that are unplayed.
/// When `threshold` is Some, only seasons with at least one released episode are
/// counted — seasons where all episodes are unreleased (upcoming seasons) are excluded
/// so they don't block cascading to the series.
async fn count_unplayed_released_seasons(
    db: &SqlitePool,
    series_id: Uuid,
    user_id: Uuid,
    threshold: Option<chrono::NaiveDateTime>,
) -> i64 {
    let mut qb =
        sqlx::QueryBuilder::new("SELECT COUNT(*) FROM media s WHERE s.parent_id = ");
    qb.push_bind(series_id);
    qb.push(" AND s.kind = 'season'");
    qb.push(
        " AND NOT EXISTS (\
           SELECT 1 FROM user_media_state ums \
           WHERE ums.media_id = s.id AND ums.user_id = ",
    );
    qb.push_bind(user_id);
    qb.push(" AND ums.play_count > 0)");
    if let Some(t) = threshold {
        qb.push(
            " AND EXISTS (\
               SELECT 1 FROM media e WHERE e.parent_id = s.id AND e.kind = 'episode'",
        );
        push_release_date_filter(&mut qb, "e", t, true);
        qb.push(")");
    }
    qb.build_query_scalar()
        .fetch_one(db)
        .await
        .unwrap_or(1)
}

async fn unplay_parent_if_played(
    db: &SqlitePool,
    user: &super::User,
    parent_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let Some(id) = parent_id else {
        return Ok(());
    };
    if let Ok(Some(parent)) = Media::get_by_id(db, &id).await {
        let ss = super::UserMediaState::get_or_new(db, user, &parent).await?;
        if ss.play_count > 0 {
            parent
                .apply_unplayed(db, user)
                .await?;
        }
    }
    Ok(())
}

async fn child_episode_ids(
    db: &SqlitePool,
    parent_id: Uuid,
    threshold: Option<chrono::NaiveDateTime>,
) -> Vec<Uuid> {
    let mut qb = sqlx::QueryBuilder::new("SELECT id FROM media WHERE parent_id = ");
    qb.push_bind(parent_id);
    qb.push(" AND kind = 'episode'");
    if let Some(t) = threshold {
        push_release_date_filter(&mut qb, "media", t, true);
    }
    qb.build_query_scalar()
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

async fn child_season_ids(
    db: &SqlitePool,
    parent_id: Uuid,
    threshold: Option<chrono::NaiveDateTime>,
) -> Vec<Uuid> {
    let mut qb = sqlx::QueryBuilder::new("SELECT id FROM media WHERE parent_id = ");
    qb.push_bind(parent_id);
    qb.push(" AND kind = 'season'");
    if let Some(t) = threshold {
        // Only seasons that have at least one released episode.
        qb.push(
            " AND EXISTS (\
               SELECT 1 FROM media e WHERE e.parent_id = media.id AND e.kind = 'episode'",
        );
        push_release_date_filter(&mut qb, "e", t, true);
        qb.push(")");
    }
    qb.build_query_scalar()
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

async fn grandchild_episode_ids(
    db: &SqlitePool,
    grandparent_id: Uuid,
    threshold: Option<chrono::NaiveDateTime>,
) -> Vec<Uuid> {
    let mut qb =
        sqlx::QueryBuilder::new("SELECT id FROM media WHERE grandparent_id = ");
    qb.push_bind(grandparent_id);
    qb.push(" AND kind = 'episode'");
    if let Some(t) = threshold {
        push_release_date_filter(&mut qb, "media", t, true);
    }
    qb.build_query_scalar()
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

/// Bulk-upsert `user_media_state` rows for `media_ids` as played (play_count = 1, played_at = `now`).
/// Existing rows with `play_count > 0` are left untouched (we only bump rows at zero).
/// New rows are inserted; existing played rows are not regressed.
async fn bulk_mark_played(
    db: &SqlitePool,
    user_id: Uuid,
    media_ids: &[Uuid],
    now: chrono::NaiveDateTime,
) {
    if media_ids.is_empty() {
        return;
    }
    for chunk in media_ids.chunks(CHUNK_SIZE) {
        // Build the media_raw JSON for each id by querying the media table, then upsert.
        // For efficiency we do a single INSERT OR REPLACE per chunk using a VALUES list.
        // We use INSERT OR REPLACE so that rows with play_count=0 are overwritten.
        // Rows that already have play_count > 0 are left alone via the CASE expression.
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO user_media_state \
             (user_id, media_id, media_raw, stream_id, favorite, play_count, played_at, \
              playback_position, last_played_at, subtitle_idx, audio_idx) \
             SELECT \
               um.user_id, m.id, NULL, NULL, \
               COALESCE((SELECT favorite FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id), 0), \
               CASE WHEN COALESCE((SELECT play_count FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id), 0) > 0 \
                    THEN (SELECT play_count FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id) \
                    ELSE 1 END, \
               CASE WHEN COALESCE((SELECT play_count FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id), 0) > 0 \
                    THEN (SELECT played_at FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id) \
                    ELSE ",
        );
        qb.push_bind(now);
        qb.push(
            " END, \
               0, \
               ",
        );
        qb.push_bind(now);
        qb.push(
            ", \
               (SELECT subtitle_idx FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id), \
               (SELECT audio_idx FROM user_media_state WHERE user_id = um.user_id AND media_id = m.id) \
             FROM (SELECT ",
        );
        qb.push_bind(user_id);
        qb.push(" AS user_id) um CROSS JOIN media m WHERE m.id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") ON CONFLICT(user_id, media_id) DO UPDATE SET \
               play_count = CASE WHEN user_media_state.play_count > 0 THEN user_media_state.play_count ELSE excluded.play_count END, \
               played_at  = CASE WHEN user_media_state.play_count > 0 THEN user_media_state.played_at  ELSE excluded.played_at  END, \
               last_played_at = excluded.last_played_at");
        if let Err(e) = qb
            .build()
            .execute(db)
            .await
        {
            warn!(error = %e, "bulk_mark_played failed for chunk");
        }
    }
}

/// Bulk-reset `user_media_state` rows for `media_ids` to unplayed state
/// (play_count = 0, played_at = NULL, playback_position = 0).
/// Only existing rows are updated; missing rows are already "unplayed" by definition.
async fn bulk_mark_unplayed(db: &SqlitePool, user_id: Uuid, media_ids: &[Uuid]) {
    if media_ids.is_empty() {
        return;
    }
    for chunk in media_ids.chunks(SQLITE_VAR_LIMIT) {
        let mut qb = sqlx::QueryBuilder::new(
            "UPDATE user_media_state SET play_count = 0, played_at = NULL, playback_position = 0 \
             WHERE user_id = ",
        );
        qb.push_bind(user_id);
        qb.push(" AND media_id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        if let Err(e) = qb
            .build()
            .execute(db)
            .await
        {
            warn!(error = %e, "bulk_mark_unplayed failed for chunk");
        }
    }
}

/// After importing episodes for a series, ensure users who had the series marked played
/// still have a consistent state. If new (released) episodes exist that aren't yet played,
/// clear the played flag on the series (and any affected seasons) for those users.
/// Unreleased episodes are excluded from the staleness check — they should not cause the
/// series to be unmarked when the user has watched everything available.
pub async fn reconcile_series_played_state(db: &SqlitePool, series_id: Uuid) {
    let threshold = super::Settings::get_config_or_default(db)
        .await
        .release_date_threshold();

    // Users who have the series played but have at least one unplayed released episode.
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT ums.user_id \
         FROM user_media_state ums \
         WHERE ums.media_id = ",
    );
    qb.push_bind(series_id);
    qb.push(
        " AND ums.play_count > 0 \
         AND EXISTS (\
           SELECT 1 FROM media e \
           WHERE e.grandparent_id = ",
    );
    qb.push_bind(series_id);
    qb.push(" AND e.kind = 'episode'");
    if let Some(t) = threshold {
        push_release_date_filter(&mut qb, "e", t, true);
    }
    qb.push(
        " AND NOT EXISTS (\
           SELECT 1 FROM user_media_state u2 \
           WHERE u2.media_id = e.id AND u2.user_id = ums.user_id AND u2.play_count > 0\
         ))",
    );
    let stale_users: Vec<Uuid> = qb
        .build_query_scalar()
        .fetch_all(db)
        .await
        .unwrap_or_default();

    for user_id in stale_users {
        // Unmark the series.
        sqlx::query(
            "UPDATE user_media_state SET play_count = 0, played_at = NULL \
             WHERE user_id = ? AND media_id = ?",
        )
        .bind(user_id)
        .bind(series_id)
        .execute(db)
        .await
        .ok();

        // Unmark any seasons that are played but contain unplayed released episodes.
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT s.id FROM media s \
             WHERE s.parent_id = ",
        );
        qb.push_bind(series_id);
        qb.push(
            " AND s.kind = 'season' \
             AND EXISTS (\
               SELECT 1 FROM user_media_state ums \
               WHERE ums.media_id = s.id AND ums.user_id = ",
        );
        qb.push_bind(user_id);
        qb.push(
            " AND ums.play_count > 0\
             ) \
             AND EXISTS (\
               SELECT 1 FROM media e \
               WHERE e.parent_id = s.id AND e.kind = 'episode'",
        );
        if let Some(t) = threshold {
            push_release_date_filter(&mut qb, "e", t, true);
        }
        qb.push(
            " AND NOT EXISTS (\
               SELECT 1 FROM user_media_state u2 \
               WHERE u2.media_id = e.id AND u2.user_id = ",
        );
        qb.push_bind(user_id);
        qb.push(" AND u2.play_count > 0))");
        let stale_seasons: Vec<Uuid> = qb
            .build_query_scalar()
            .fetch_all(db)
            .await
            .unwrap_or_default();

        for season_id in stale_seasons {
            sqlx::query(
                "UPDATE user_media_state SET play_count = 0, played_at = NULL \
                 WHERE user_id = ? AND media_id = ?",
            )
            .bind(user_id)
            .bind(season_id)
            .execute(db)
            .await
            .ok();
        }
    }
}

impl From<sdks::stremio::Catalog> for Media {
    fn from(source: sdks::stremio::Catalog) -> Self {
        Media {
            title: source.name,
            kind: MediaKind::Collection,
            ..Default::default()
        }
    }
}

impl From<sdks::stremio::Stream> for Media {
    fn from(source: sdks::stremio::Stream) -> Self {
        use crate::stream::{StreamDescriptor, StreamInfo};
        let descriptor = if let Some(hash) = &source.info_hash {
            StreamDescriptor::Torrent {
                info_hash: hash.to_ascii_lowercase(),
                file_hint: source
                    .filename
                    .clone(),
                file_idx: source
                    .file_idx
                    .map(|i| i as usize),
                trackers: source
                    .sources
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|src| src.strip_prefix("tracker:"))
                    .map(String::from)
                    .collect(),
            }
        } else if let Some(url) = source
            .url
            .clone()
            .or_else(|| {
                source
                    .external_url
                    .clone()
            })
        {
            StreamDescriptor::http(url)
        } else {
            return Media {
                kind: MediaKind::Stream,
                id: source.get_guid(),
                ..Default::default()
            };
        };

        let stream_info = Some(StreamInfo {
            descriptor,
            filename: source
                .filename
                .clone(),
            name: source
                .name
                .clone(),
            description: source
                .description
                .clone(),
            seeders: source.seeders,
            size: source.size,
            duration: source.duration,
            subtitles: source
                .subtitles
                .clone(),
            probe_data: None,
            source: None,
            catchup_source: None,
            catchup_days: None,
        });

        // Merge name + description: AIOStreams puts the provider/addon name in `name`
        // and the full codec/resolution details in `description`. Clients expect both.
        let title = match (&source.name, &source.description) {
            (Some(n), Some(d)) if !d.is_empty() => format!("{}\n{}", n, d),
            (Some(n), _) => n.clone(),
            (None, Some(d)) => d.clone(),
            _ => String::new(),
        };

        Media {
            title,
            kind: MediaKind::Stream,
            stream_info,
            id: source.get_guid(),
            ..Default::default()
        }
    }
}

impl TryFrom<sdks::stremio::Meta> for Media {
    type Error = anyhow::Error;
    fn try_from(meta: sdks::stremio::Meta) -> Result<Media> {
        //self.info_hash.is_some()
        // let imdb_id = meta.imdb_id.context("missing IMDB ID")?;

        let mut media_kind = MediaKind::try_from(
            meta.media_type
                .clone(),
        )
        .unwrap_or(MediaKind::Movie);
        if media_kind == MediaKind::Movie
            && meta
                .videos
                .as_ref()
                .map_or(false, |v| !v.is_empty())
        {
            media_kind = MediaKind::Series;
        }

        let digital_released_at = meta
            .app_extras
            .as_ref()
            .and_then(|e| {
                e.release_dates
                    .as_ref()
            })
            .map(|rd| {
                {
                    rd.results
                        .iter()
                        .flat_map(|country| {
                            country
                                .release_dates
                                .iter()
                        })
                        .filter(|entry| entry.release_type >= 4)
                        .map(|entry| entry.release_date)
                        .min()
                }
            })
            .flatten()
            .map(|dt| dt.naive_utc())
            // Series/seasons/episodes use their air date as the digital release date
            // when TMDB release_dates are not available.
            .or_else(|| {
                if matches!(
                    media_kind,
                    MediaKind::Series | MediaKind::Season | MediaKind::Episode
                ) {
                    meta.released
                        .map(|x| x.naive_utc())
                } else {
                    None
                }
            });

        let status =
            meta.status
                .as_ref()
                .map(|s| match s {
                    sdks::stremio::Status::Continuing
                    | sdks::stremio::Status::ReturningSeries
                    | sdks::stremio::Status::InProduction
                    | sdks::stremio::Status::Running => MediaStatus::Continuing,
                    sdks::stremio::Status::Ended | sdks::stremio::Status::Canceled => {
                        MediaStatus::Ended
                    }
                    sdks::stremio::Status::Upcoming
                    | sdks::stremio::Status::Planned => MediaStatus::Unreleased,
                    sdks::stremio::Status::Unknown => MediaStatus::Continuing,
                });

        let media = Media {
            title: meta
                .get_name()
                .unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta
                .released
                .map(|x| x.naive_utc()),
            digital_released_at,
            runtime: meta
                .runtime
                .map(|d| d.num_seconds()),
            // rating_critic: meta.rating_critic,
            rating_audience: meta.imdb_rating,
            description: meta.description,
            certification: meta
                .certification
                .clone(),
            certification_age: {
                let country = meta
                    .country
                    .as_ref()
                    .and_then(|v| v.first())
                    .map(|c| normalize_country_alpha2(c));
                crate::localization::ratings::resolve_rating_age(
                    meta.certification
                        .as_deref(),
                    country.as_deref(),
                )
            },
            country: meta
                .country
                .and_then(|v| {
                    v.into_iter()
                        .next()
                })
                .map(|c| normalize_country_alpha2(&c)),
            external_ids: {
                let mut ids = ExternalIds::from_stremio_id(&meta.id);
                if let Some(ref imdb) = meta.imdb_id {
                    ids.imdb = NonEmptyString::try_new(imdb.clone()).ok();
                }
                ids
            },
            status,
            trailers: meta
                .trailers
                .map(|trailers| {
                    trailers
                        .into_iter()
                        .map(|t| t.source)
                        .collect::<Vec<String>>()
                }),
            id: {
                // Prefer the explicit imdb_id field; fall back to extracting it from
                // meta.id (e.g. Cinemeta returns id="tt0076759" without imdb_id set).
                let imdb_id: Option<NonEmptyString> = meta
                    .imdb_id
                    .as_deref()
                    .and_then(|s| NonEmptyString::try_new(s.to_string()).ok())
                    .or_else(|| ExternalIds::from_stremio_id(&meta.id).imdb);
                imdb_id
                    .as_ref()
                    .map(|mid| {
                        Uuid::from(&super::MediaIdRaw {
                            kind: media_kind.clone(),
                            external_ids: ExternalIds {
                                imdb: Some(mid.clone()),
                                ..Default::default()
                            },
                            season: None,
                            episode: None,
                        })
                    })
                    .unwrap_or_else(|| {
                        // No IMDB ID extractable yet — use a deterministic UUID from
                        // the raw Stremio ID as a temporary store key. persist_from_store
                        // will recompute the correct stable UUID after IMDB resolution.
                        // If this ever reaches upsert unresolved, validate() rejects it.
                        crate::common::stable_media_uuid(&media_kind, &meta.id)
                    })
            },
            ..Default::default()
        };

        let mut media = media;
        if let Some(url) = meta
            .poster
            .or(meta.thumbnail)
        {
            media.set_image(ImageKind::Primary, url);
        }
        if let Some(url) = meta.logo {
            media.set_image(ImageKind::Logo, url);
        }
        if let Some(url) = meta.background {
            media.set_image(ImageKind::Backdrop, url);
        }

        Ok(media)
    }
}

pub fn stremio_meta_to_medias(meta: sdks::stremio::Meta) -> Result<Vec<Media>> {
    let imdb_id: Option<NonEmptyString> = meta
        .imdb_id
        .as_deref()
        .and_then(|s| NonEmptyString::try_new(s.to_string()).ok());

    let mut media: Media = meta
        .clone()
        .try_into()?;

    if imdb_id.is_none() {
        // Custom-ID path: no IMDB, derive UUIDs from the addon-specific id.
        let custom_id = ExternalIds::from_stremio_id(&meta.id)
            .custom_stremio_id
            .context("imdb_id is missing and meta.id is empty")?;
        media.id = Uuid::from(&super::MediaIdRaw {
            kind: media
                .kind
                .clone(),
            external_ids: ExternalIds {
                custom_stremio_id: Some(custom_id.clone()),
                ..Default::default()
            },
            season: None,
            episode: None,
        });
        media
            .external_ids
            .custom_stremio_id = Some(custom_id.clone());
        let mut media_instances = vec![media.clone()];
        if let MediaKind::Series = media.kind {
            if let Some(ref episodes) = meta.videos {
                let seasons: std::collections::BTreeMap<
                    i64,
                    Vec<sdks::stremio::Episode>,
                > = episodes
                    .iter()
                    .filter_map(|ep| {
                        ep.season
                            .map(|s| (s, ep.clone()))
                    })
                    .fold(std::collections::BTreeMap::new(), |mut acc, (s, ep)| {
                        acc.entry(s)
                            .or_default()
                            .push(ep);
                        acc
                    });
                for (season_idx, episodes) in seasons {
                    let season_id = Uuid::from(&super::MediaIdRaw {
                        kind: MediaKind::Season,
                        external_ids: ExternalIds {
                            series_custom_stremio_id: Some(custom_id.clone()),
                            ..Default::default()
                        },
                        season: Some(season_idx),
                        episode: None,
                    });
                    let mut season = Media {
                        id: season_id,
                        title: format!("Season {}", season_idx),
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        parent_id: Some(media.id),
                        grandparent_id: Some(media.id),
                        external_ids: ExternalIds {
                            series_custom_stremio_id: Some(custom_id.clone()),
                            ..Default::default()
                        },
                        released_at: episodes
                            .first()
                            .and_then(|e| e.released)
                            .map(|x| x.naive_utc()),
                        digital_released_at: episodes
                            .first()
                            .and_then(|e| e.released)
                            .map(|x| x.naive_utc()),
                        ..Default::default()
                    };
                    if let Some(url) = meta.get_season_poster(season_idx) {
                        season.set_image(ImageKind::Primary, url);
                    }
                    media_instances.push(season);
                    for ep in episodes {
                        let ep_idx = ep
                            .episode
                            .unwrap_or(0);
                        let mut episode: Media = ep
                            .clone()
                            .try_into()?;
                        episode.id = Uuid::from(&super::MediaIdRaw {
                            kind: MediaKind::Episode,
                            external_ids: ExternalIds {
                                series_custom_stremio_id: Some(custom_id.clone()),
                                ..Default::default()
                            },
                            season: Some(season_idx),
                            episode: Some(ep_idx),
                        });
                        episode.idx = ep.episode;
                        episode.external_ids = ExternalIds {
                            series_custom_stremio_id: Some(custom_id.clone()),
                            ..Default::default()
                        };
                        episode.parent_id = Some(season_id);
                        episode.grandparent_id = Some(media.id);
                        episode.parent_idx = Some(season_idx);
                        episode.released_at = ep
                            .released
                            .map(|x| x.naive_utc());
                        episode.digital_released_at = ep
                            .released
                            .map(|x| x.naive_utc());
                        media_instances.push(episode);
                    }
                }
            }
        }
        return Ok(media_instances);
    }

    let imdb_id = imdb_id.unwrap();

    media.id = Uuid::from(&super::MediaIdRaw {
        kind: media
            .kind
            .clone(),
        external_ids: ExternalIds {
            imdb: Some(imdb_id.clone()),
            ..Default::default()
        },
        season: None,
        episode: None,
    });

    let mut media_instances = Vec::new();
    media_instances.push(media.clone());

    if let MediaKind::Series = media.kind {
        if let Some(ref episodes) = meta.videos {
            let seasons: std::collections::BTreeMap<i64, Vec<sdks::stremio::Episode>> =
                episodes
                    .iter()
                    .filter_map(|ep| {
                        ep.season
                            .map(|s| (s, ep.clone()))
                    })
                    .fold(
                        std::collections::BTreeMap::new(),
                        |mut acc, (season, ep)| {
                            acc.entry(season)
                                .or_default()
                                .push(ep);
                            acc
                        },
                    );
            for (season_idx, episodes) in seasons {
                let mut season = Media {
                    id: Uuid::from(&super::MediaIdRaw {
                        kind: MediaKind::Season,
                        external_ids: ExternalIds {
                            series_imdb: Some(imdb_id.clone()),
                            ..Default::default()
                        },
                        season: Some(season_idx),
                        episode: None,
                    }),
                    title: format!("Season {}", season_idx),
                    kind: MediaKind::Season,
                    idx: Some(season_idx),
                    grandparent_id: Some(media.id),
                    external_ids: ExternalIds {
                        series_imdb: Some(imdb_id.clone()),
                        series_tmdb: media
                            .external_ids
                            .tmdb,
                        ..Default::default()
                    },
                    parent_id: Some(media.id),
                    released_at: episodes
                        .first()
                        .and_then(|e| e.released)
                        .map(|x| x.naive_utc()),
                    digital_released_at: episodes
                        .first()
                        .and_then(|e| e.released)
                        .map(|x| x.naive_utc()),
                    ..Default::default()
                };
                if let Some(url) = meta.get_season_poster(season_idx) {
                    season.set_image(ImageKind::Primary, url);
                }
                media_instances.push(season.clone());

                for ep in episodes {
                    let mut episode: Media = ep
                        .clone()
                        .try_into()?;
                    let ep_idx = ep
                        .episode
                        .unwrap_or(0);
                    episode.id = Uuid::from(&super::MediaIdRaw {
                        kind: MediaKind::Episode,
                        external_ids: ExternalIds {
                            series_imdb: Some(imdb_id.clone()),
                            ..Default::default()
                        },
                        season: Some(season_idx),
                        episode: Some(ep_idx),
                    });
                    episode.idx = ep.episode;
                    episode.external_ids = ExternalIds {
                        series_imdb: Some(imdb_id.clone()),
                        series_tmdb: media
                            .external_ids
                            .tmdb,
                        ..Default::default()
                    };
                    episode.grandparent_id = Some(media.id);
                    episode.parent_id = Some(season.id);
                    episode.parent_idx = Some(season_idx);
                    episode.released_at = ep
                        .released
                        .map(|x| x.naive_utc());
                    episode.digital_released_at = ep
                        .released
                        .map(|x| x.naive_utc());

                    let rels = build_episode_relations_from_ep(&episode, &ep);
                    if !rels.is_empty() {
                        episode.relations = Some(rels);
                    }

                    media_instances.push(episode);
                }
            }
        }
    }

    Ok(media_instances)
}

/// Extracts season-level `Media` items from a cached Stremio `Meta` without cloning
/// the full response. Used by the streaming tree path where episodes are fetched
/// per-season rather than all-at-once.
pub fn stremio_meta_seasons(
    meta: &crate::sdks::stremio::Meta,
    series_id: Uuid,
    series_external_ids: &ExternalIds,
) -> Vec<Media> {
    let imdb_id = series_external_ids
        .imdb
        .clone();
    let custom_id = series_external_ids
        .custom_stremio_id
        .clone();

    let Some(videos) = meta
        .videos
        .as_ref()
    else {
        return vec![];
    };

    // Collect unique season numbers with their first episode's release date.
    let mut seasons_map: std::collections::BTreeMap<
        i64,
        &crate::sdks::stremio::Episode,
    > = std::collections::BTreeMap::new();
    for ep in videos {
        if let Some(s) = ep.season {
            seasons_map
                .entry(s)
                .or_insert(ep);
        }
    }

    let mut out = Vec::with_capacity(seasons_map.len());
    for (season_idx, first_ep) in seasons_map {
        let (season_id, external_ids) = if let Some(ref iid) = imdb_id {
            let id = Uuid::from(&super::MediaIdRaw {
                kind: MediaKind::Season,
                external_ids: ExternalIds {
                    series_imdb: Some(iid.clone()),
                    ..Default::default()
                },
                season: Some(season_idx),
                episode: None,
            });
            let ext = ExternalIds {
                series_imdb: Some(iid.clone()),
                series_tmdb: series_external_ids.tmdb,
                ..Default::default()
            };
            (id, ext)
        } else if let Some(ref cid) = custom_id {
            let id = Uuid::from(&super::MediaIdRaw {
                kind: MediaKind::Season,
                external_ids: ExternalIds {
                    series_custom_stremio_id: Some(cid.clone()),
                    ..Default::default()
                },
                season: Some(season_idx),
                episode: None,
            });
            let ext = ExternalIds {
                series_custom_stremio_id: Some(cid.clone()),
                ..Default::default()
            };
            (id, ext)
        } else {
            continue;
        };

        let mut season = Media {
            id: season_id,
            title: format!("Season {}", season_idx),
            kind: MediaKind::Season,
            idx: Some(season_idx),
            parent_id: Some(series_id),
            grandparent_id: Some(series_id),
            external_ids,
            released_at: first_ep
                .released
                .map(|x| x.naive_utc()),
            digital_released_at: first_ep
                .released
                .map(|x| x.naive_utc()),
            ..Default::default()
        };
        if let Some(url) = meta.get_season_poster(season_idx) {
            season.set_image(ImageKind::Primary, url);
        }
        out.push(season);
    }
    out
}

/// Extracts episode-level `Media` items for a single season from a cached Stremio `Meta`.
/// Only the target season's videos are converted, keeping per-iteration allocations small.
pub fn stremio_meta_season_episodes(
    meta: &crate::sdks::stremio::Meta,
    series_id: Uuid,
    season_id: Uuid,
    season_idx: i64,
    series_external_ids: &ExternalIds,
) -> Result<Vec<Media>> {
    let imdb_id = series_external_ids
        .imdb
        .clone();
    let custom_id = series_external_ids
        .custom_stremio_id
        .clone();

    let Some(videos) = meta
        .videos
        .as_ref()
    else {
        return Ok(vec![]);
    };

    let mut out = Vec::new();
    for ep in videos
        .iter()
        .filter(|e| e.season == Some(season_idx))
    {
        let ep_idx = ep
            .episode
            .unwrap_or(0);
        let mut episode: Media = ep
            .clone()
            .try_into()?;

        if let Some(ref iid) = imdb_id {
            episode.id = Uuid::from(&super::MediaIdRaw {
                kind: MediaKind::Episode,
                external_ids: ExternalIds {
                    series_imdb: Some(iid.clone()),
                    ..Default::default()
                },
                season: Some(season_idx),
                episode: Some(ep_idx),
            });
            episode.external_ids = ExternalIds {
                series_imdb: Some(iid.clone()),
                series_tmdb: series_external_ids.tmdb,
                ..Default::default()
            };
        } else if let Some(ref cid) = custom_id {
            episode.id = Uuid::from(&super::MediaIdRaw {
                kind: MediaKind::Episode,
                external_ids: ExternalIds {
                    series_custom_stremio_id: Some(cid.clone()),
                    ..Default::default()
                },
                season: Some(season_idx),
                episode: Some(ep_idx),
            });
            episode.external_ids = ExternalIds {
                series_custom_stremio_id: Some(cid.clone()),
                ..Default::default()
            };
        }

        episode.idx = ep.episode;
        episode.parent_idx = Some(season_idx);
        episode.parent_id = Some(season_id);
        episode.grandparent_id = Some(series_id);
        episode.released_at = ep
            .released
            .map(|x| x.naive_utc());
        episode.digital_released_at = ep
            .released
            .map(|x| x.naive_utc());

        let rels = build_episode_relations_from_ep(&episode, ep);
        if !rels.is_empty() {
            episode.relations = Some(rels);
        }

        out.push(episode);
    }
    Ok(out)
}

/// Return the release-date WHERE fragment for use in raw `format!` SQL strings.
/// Push the release-date WHERE condition onto a query builder, binding `threshold`.
///
/// `alias` is the table alias for the media row (e.g. `"media"` for an unaliased
/// table, `"e"` when episodes are selected as `media e`).
///
/// Appends a WHERE condition that hides items whose resolved release date is after `threshold`.
///
/// Resolution priority (CASE expression):
/// 1. `digital_released_at` — explicit digital/streaming date; used as-is.
/// 2. `released_at` within the past year → NULL (hidden). A recent theatrical release
///    with no digital date confirmed is still considered unreleased digitally.
/// 3. ELSE — depends on `use_parent_fallback`:
///    - `true`  (episodes, series, movies): fall back to the parent row's dates via a
///      correlated subquery. This lets undated episodes of old series (e.g. a 1990s
///      show imported from Jellyfin with no per-episode air dates) inherit the series
///      premiere and be treated as released rather than silently disappearing.
///    - `false` (seasons): no parent fallback. A season with no own dates returns NULL
///      from the CASE, which fails `<= threshold` and is hidden. This is intentional:
///      TVDB often lists upcoming seasons before scheduling them, and we must not let
///      such a season inherit the series' past premiere date and slip through the filter.
///
/// In all cases a NULL result from the CASE is falsy in SQLite (`NULL <= x` = NULL),
/// so items that cannot resolve any date are excluded.
pub fn push_release_date_filter(
    qb: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    alias: &str,
    threshold: NaiveDateTime,
    use_parent_fallback: bool,
) {
    let a = format!("{alias}.");
    let else_expr = if use_parent_fallback {
        format!(
            "COALESCE(\
              {a}released_at, \
              (SELECT COALESCE(p.digital_released_at, p.released_at) FROM media p WHERE p.id = {a}parent_id)\
            )"
        )
    } else {
        format!("{a}released_at")
    };
    qb.push(format!(
        " AND CASE \
            WHEN {a}digital_released_at IS NOT NULL THEN {a}digital_released_at \
            WHEN {a}released_at IS NOT NULL AND datetime({a}released_at) > datetime('now', '-1 year') THEN NULL \
            ELSE {else_expr} \
          END <= "
    ))
    .push_bind(threshold);
}

/// Append WHERE clauses for a set of `FilterRule`s onto a query builder.
///
/// Called once for both the count and records builders inside `get_by_filter`.
///
/// # SQL strategy per field
/// - `year` / `rating_*` / `certification` — direct column comparison
/// - `tag` — EXISTS in `media_tags`
/// - `genre` / `studio` — EXISTS in `media_relations` joining by title
/// - `catalog` — EXISTS in `media_relations` with `role = 'catalog'` joining by title
/// - `has_trailer` — json_array_length check
pub fn apply_filter_rules(
    qb: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    filter: &remux_sdks::remux::CollectionFilter,
) {
    use remux_sdks::remux::FilterMatchMode;

    let non_empty: Vec<_> = filter
        .groups
        .iter()
        .filter(|g| {
            !g.rules
                .is_empty()
        })
        .collect();

    if non_empty.is_empty() {
        return;
    }

    let group_sep = match filter.match_mode {
        FilterMatchMode::All => " AND ",
        FilterMatchMode::Any => " OR ",
    };

    qb.push(" AND (");
    let mut first_group = true;
    for group in non_empty {
        if !first_group {
            qb.push(group_sep);
        }
        first_group = false;

        let rule_sep = match group.match_mode {
            FilterMatchMode::All => " AND ",
            FilterMatchMode::Any => " OR ",
        };

        qb.push("(");
        let mut first_rule = true;
        for rule in &group.rules {
            if let Some((sql, negated)) = filter_rule_to_sql(rule) {
                if !first_rule {
                    qb.push(rule_sep);
                }
                first_rule = false;
                if negated {
                    qb.push("NOT (");
                }
                qb.push(sql);
                if negated {
                    qb.push(")");
                }
            }
        }
        qb.push(")");
    }
    qb.push(")");
}

/// Translate one `FilterRule` into a raw SQL fragment.
///
/// Values are embedded directly — no string parsing needed since the rule carries typed values.
/// Returns `(sql, negated)` — caller wraps in `NOT(...)` when negated is true.
/// Returns `None` if the rule should be skipped (e.g. empty values list).
fn filter_rule_to_sql(rule: &remux_sdks::remux::FilterRule) -> Option<(String, bool)> {
    use remux_sdks::remux::{FilterRule as R, NumericOp, SetOp};

    fn esc(s: &str) -> String {
        s.replace('\'', "''")
    }

    fn in_list(values: &[String]) -> Option<String> {
        let items: Vec<String> = values
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("lower('{}')", esc(s)))
            .collect();
        if items.is_empty() {
            return None;
        }
        Some(items.join(", "))
    }

    match rule {
        R::Year { op, value } => {
            let negated = *op == NumericOp::NotEq;
            let sql = match op {
                NumericOp::Eq | NumericOp::NotEq => {
                    format!("CAST(strftime('%Y', released_at) AS INTEGER) = {value}")
                }
                NumericOp::Gt => {
                    format!("CAST(strftime('%Y', released_at) AS INTEGER) > {value}")
                }
                NumericOp::Lt => {
                    format!("CAST(strftime('%Y', released_at) AS INTEGER) < {value}")
                }
            };
            Some((sql, negated))
        }
        R::RatingAudience { op, value } => {
            let negated = *op == NumericOp::NotEq;
            let sql = match op {
                NumericOp::Eq | NumericOp::NotEq => {
                    format!("rating_audience = {value}")
                }
                NumericOp::Gt => format!("rating_audience > {value}"),
                NumericOp::Lt => format!("rating_audience < {value}"),
            };
            Some((sql, negated))
        }
        R::RatingCritic { op, value } => {
            let negated = *op == NumericOp::NotEq;
            let sql = match op {
                NumericOp::Eq | NumericOp::NotEq => format!("rating_critic = {value}"),
                NumericOp::Gt => format!("rating_critic > {value}"),
                NumericOp::Lt => format!("rating_critic < {value}"),
            };
            Some((sql, negated))
        }
        R::ParentalRating { op, value } => {
            let negated = *op == NumericOp::NotEq;
            let sql = match op {
                NumericOp::Eq | NumericOp::NotEq => {
                    format!("certification_age = {value}")
                }
                NumericOp::Gt => format!("certification_age > {value}"),
                NumericOp::Lt => format!("certification_age <= {value}"),
            };
            Some((sql, negated))
        }
        R::Certification { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!("lower(certification) = lower('{v}')")
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!("lower(certification) IN ({list})")
                }
            };
            Some((sql, negated))
        }
        R::Country { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'country' AND lower(title) = lower('{v}')))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'country' AND lower(title) IN ({list})))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::OriginalLanguage { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!("lower(original_language) = lower('{v}')")
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!("lower(original_language) IN ({list})")
                }
            };
            Some((sql, negated))
        }
        R::Tag { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!(
                        "EXISTS (SELECT 1 FROM media_tags mt WHERE mt.media_id = media.id AND lower(mt.tag) = lower('{v}'))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "EXISTS (SELECT 1 FROM media_tags mt WHERE mt.media_id = media.id AND lower(mt.tag) IN ({list}))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::Genre { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'genre' AND lower(title) = lower('{v}')))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'genre' AND lower(title) IN ({list})))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::Studio { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'studio' AND lower(title) = lower('{v}')))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         WHERE mr.right_media_id IN \
                         (SELECT id FROM media WHERE kind = 'studio' AND lower(title) IN ({list})))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::HasTrailer { value } => {
            let sql = if *value {
                "json_array_length(trailers) > 0".to_string()
            } else {
                "(trailers IS NULL OR json_array_length(trailers) = 0)".to_string()
            };
            Some((sql, false))
        }
        R::Person { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or(""));
                    format!(
                        "EXISTS (SELECT 1 FROM media_relations mr \
                         JOIN media p ON p.id = mr.right_media_id \
                         WHERE mr.left_media_id = media.id AND p.kind = 'person' AND lower(p.title) = lower('{v}'))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "EXISTS (SELECT 1 FROM media_relations mr \
                         JOIN media p ON p.id = mr.right_media_id \
                         WHERE mr.left_media_id = media.id AND p.kind = 'person' AND lower(p.title) IN ({list}))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::Catalog { op, catalog_ids } if !catalog_ids.is_empty() => {
            let in_clause = catalog_ids
                .iter()
                .map(|id| format!("X'{}'", id.simple()))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "EXISTS (SELECT 1 FROM media_relations mr \
                 WHERE mr.right_media_id = media.id AND mr.role = 'catalog' \
                 AND mr.left_media_id IN ({in_clause}))"
            );
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            Some((sql, negated))
        }
        R::Catalog { .. } => None,
    }
}

fn build_episode_relations_from_ep(
    media: &Media,
    ep: &crate::sdks::stremio::Episode,
) -> Vec<(MediaRelation, Media)> {
    let mut relations = Vec::new();
    let add_names = |relations: &mut Vec<(MediaRelation, Media)>,
                     names: Option<&Vec<String>>,
                     role: RelationRole| {
        let names: Vec<String> = names
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|s| {
                s.split(',')
                    .map(|n| {
                        n.trim()
                            .to_string()
                    })
            })
            .filter(|s| !s.is_empty())
            .collect();
        for (i, name) in names
            .into_iter()
            .enumerate()
        {
            let person_id = crate::common::stable_media_uuid(
                &MediaKind::Person,
                &name.to_lowercase(),
            );
            relations.push((
                MediaRelation {
                    left_media_id: media.id,
                    right_media_id: person_id,
                    weight: Some(i as i64),
                    role: Some(role.clone()),
                    ..Default::default()
                },
                Media {
                    id: person_id,
                    title: name.clone(),
                    kind: MediaKind::Person,
                    ..Default::default()
                },
            ));
        }
    };
    add_names(
        &mut relations,
        ep.directors
            .as_ref(),
        RelationRole::Director,
    );
    add_names(
        &mut relations,
        ep.writers
            .as_ref(),
        RelationRole::Writer,
    );
    relations
}

pub(crate) fn build_genre_relations_from_names(
    left_id: uuid::Uuid,
    names: &[String],
    kind: MediaKind,
) -> Vec<(MediaRelation, Media)> {
    names
        .iter()
        .map(|name| {
            let gid = crate::common::stable_media_uuid(&kind, &name.to_lowercase());
            (
                MediaRelation {
                    left_media_id: left_id,
                    right_media_id: gid,
                    ..Default::default()
                },
                Media {
                    id: gid,
                    title: name.clone(),
                    kind: kind.clone(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MediaIdRaw;

    #[test]
    fn stale_episode_id_recomputes_to_canonical_and_validates() {
        let series_imdb = NonEmptyString::try_new("tt1844624".to_string()).unwrap();
        let mut ep = Media {
            kind: MediaKind::Episode,
            title: "S0E1 - Behind the Fright".to_string(),
            idx: Some(1),
            parent_idx: Some(0),
            external_ids: ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            id: crate::common::stable_media_uuid(&MediaKind::Episode, "tt1844624:1:1"),
            ..Default::default()
        };

        assert!(
            ep.validate()
                .is_err()
        );

        let raw = ep.media_id_raw();
        assert!(
            raw.canonical()
                .is_some()
        );
        ep.id = Uuid::from(&raw);

        assert_eq!(
            ep.id,
            crate::common::stable_media_uuid(&MediaKind::Episode, "tt1844624:0:1")
        );
        assert!(
            ep.validate()
                .is_ok()
        );
    }

    #[test]
    fn stale_season_id_recomputes_to_canonical_and_validates() {
        let series_imdb = NonEmptyString::try_new("tt1844624".to_string()).unwrap();
        let mut season = Media {
            kind: MediaKind::Season,
            title: "Specials".to_string(),
            idx: Some(0),
            external_ids: ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            id: crate::common::stable_media_uuid(&MediaKind::Season, "tt1844624:1"),
            ..Default::default()
        };

        assert!(
            season
                .validate()
                .is_err()
        );

        let raw = season.media_id_raw();
        assert!(
            raw.canonical()
                .is_some()
        );
        season.id = Uuid::from(&raw);

        assert_eq!(
            season.id,
            crate::common::stable_media_uuid(&MediaKind::Season, "tt1844624:0")
        );
        assert!(
            season
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn from_path_tmdb_in_directory() {
        let ids = ExternalIds::from_path(
            "Movies/The Matrix (1999) [tmdbid-603]/The Matrix.mkv",
        );
        assert_eq!(ids.tmdb, Some(603));
        assert!(
            ids.imdb
                .is_none()
        );
        assert!(
            ids.tvdb
                .is_none()
        );
    }

    #[test]
    fn from_path_tvdb_in_directory() {
        let ids = ExternalIds::from_path(
            "TV/Breaking Bad [tvdbid-81189]/Season 1/S01E01.mkv",
        );
        assert_eq!(ids.tvdb, Some(81189));
        assert!(
            ids.tmdb
                .is_none()
        );
    }

    #[test]
    fn from_path_imdb_in_filename() {
        let ids = ExternalIds::from_path("[imdbid-tt0133093] The Matrix 1999.mkv");
        assert_eq!(
            ids.imdb
                .as_ref()
                .map(|s| s.as_str()),
            Some("tt0133093")
        );
    }

    #[test]
    fn from_path_short_form_tmdb() {
        let ids = ExternalIds::from_path("[tmdb-603]/movie.mkv");
        assert_eq!(ids.tmdb, Some(603));
    }

    #[test]
    fn from_path_case_insensitive() {
        let ids = ExternalIds::from_path("[TMDBID-603]/movie.mkv");
        assert_eq!(ids.tmdb, Some(603));
    }

    #[test]
    fn from_path_multiple_ids() {
        let ids = ExternalIds::from_path("Show [tmdbid-603] [tvdbid-81189]/S01E01.mkv");
        assert_eq!(ids.tmdb, Some(603));
        assert_eq!(ids.tvdb, Some(81189));
    }

    #[test]
    fn from_path_invalid_numeric_id_is_empty() {
        let ids = ExternalIds::from_path("[tmdbid-notanumber]/movie.mkv");
        assert!(ids.is_empty());
    }

    #[test]
    fn from_path_no_brackets_is_empty() {
        let ids = ExternalIds::from_path("The.Matrix.1999.mkv");
        assert!(ids.is_empty());
    }

    #[test]
    fn from_path_first_match_wins_per_field() {
        // directory has [tmdbid-603], filename repeats with a different id — first wins
        let ids = ExternalIds::from_path("[tmdbid-603]/[tmdbid-999].mkv");
        assert_eq!(ids.tmdb, Some(603));
    }

    #[test]
    fn from_path_tvdb_black_summoner() {
        let ids = ExternalIds::from_path(
            "Black Summoner (2022) [tvdbid-416588]/Season 01/Black.Summoner.S01E01.mkv",
        );
        assert_eq!(ids.tvdb, Some(416588));
        assert!(
            ids.imdb
                .is_none()
        );
        assert!(
            ids.tmdb
                .is_none()
        );
    }

    #[test]
    fn from_path_tvdb_bleach() {
        let ids = ExternalIds::from_path(
            "Bleach (2004) [tvdbid-74796]/Season 01/Bleach.S01E01.mkv",
        );
        assert_eq!(ids.tvdb, Some(74796));
        assert!(
            ids.imdb
                .is_none()
        );
        assert!(
            ids.tmdb
                .is_none()
        );
    }

    #[test]
    fn from_path_tvdb_blood_c() {
        let ids = ExternalIds::from_path(
            "Blood-C (2011) [tvdbid-249864]/Season 01/Blood-C.S01E01.mkv",
        );
        assert_eq!(ids.tvdb, Some(249864));
        assert!(
            ids.imdb
                .is_none()
        );
        assert!(
            ids.tmdb
                .is_none()
        );
    }

    /// Verifies push_release_date_filter hides movies with a recent theatrical date
    /// but no digital release date, while still showing movies with an old theatrical
    /// date (>1 year) or an explicit digital release date.
    #[tokio::test]
    async fn release_date_filter_hides_recent_theatrical_only_movies() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let now = chrono::Utc::now().naive_utc();

        let make_movie_ids = |imdb: &str| {
            let ext = ExternalIds {
                imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
                ..Default::default()
            };
            let id = uuid::Uuid::from(&MediaIdRaw {
                kind: MediaKind::Movie,
                external_ids: ext.clone(),
                season: None,
                episode: None,
            });
            (id, ext)
        };

        let (id_recent, ext_recent) = make_movie_ids("tt9990001");
        let (id_old, ext_old) = make_movie_ids("tt9990002");
        let (id_digital, ext_digital) = make_movie_ids("tt9990003");

        // Theatrical only, released 2 months ago — no digital date → must be hidden.
        let mut recent_theatrical = Media {
            id: id_recent,
            title: "Recent Theatrical Only".to_string(),
            kind: MediaKind::Movie,
            external_ids: ext_recent,
            released_at: Some(now - chrono::Duration::days(60)),
            digital_released_at: None,
            ..Default::default()
        };
        recent_theatrical
            .save(db)
            .await
            .unwrap();

        // Theatrical only, released 2 years ago — no digital date → old enough, must be shown.
        let mut old_theatrical = Media {
            id: id_old,
            title: "Old Theatrical Only".to_string(),
            kind: MediaKind::Movie,
            external_ids: ext_old,
            released_at: Some(now - chrono::Duration::days(730)),
            digital_released_at: None,
            ..Default::default()
        };
        old_theatrical
            .save(db)
            .await
            .unwrap();

        // Has explicit digital release date yesterday → must be shown.
        let mut has_digital = Media {
            id: id_digital,
            title: "Has Digital Release".to_string(),
            kind: MediaKind::Movie,
            external_ids: ext_digital,
            released_at: None,
            digital_released_at: Some(now - chrono::Duration::days(1)),
            ..Default::default()
        };
        has_digital
            .save(db)
            .await
            .unwrap();

        let result = Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![MediaKind::Movie]),
                digital_released_before: Some(now),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let titles: Vec<&str> = result
            .records
            .iter()
            .map(|m| {
                m.title
                    .as_str()
            })
            .collect();

        assert!(
            !titles.contains(&"Recent Theatrical Only"),
            "recent theatrical-only movie must be hidden; got: {:?}",
            titles
        );
        assert!(
            titles.contains(&"Old Theatrical Only"),
            "old theatrical-only movie must be shown; got: {:?}",
            titles
        );
        assert!(
            titles.contains(&"Has Digital Release"),
            "movie with digital release date must be shown; got: {:?}",
            titles
        );
    }
}
