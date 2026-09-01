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
    #[serde(rename = "unknown")]
    #[strum(to_string = "unknown", serialize = "unknown")]
    #[sqlx(rename = "unknown")]
    Other,
}

/// Deezer record type: what kind of release an Album row is. `NULL` = unknown
/// (e.g. albums imported from sources without a record type). Stored in the
/// `media.album_kind` column; the Albums view filters out Single/Ep.
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
pub enum AlbumKind {
    Album,
    Single,
    Ep,
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
)]
pub enum MetadataField {
    #[default]
    Name,
    Overview,
    Runtime,
    OfficialRating,
    Genres,
    Cast,
    Tags,
    ProductionLocations,
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

    /// Whether the kind is a directly-playable leaf item, as opposed to a
    /// container that only groups other media (albums, artists, series, ...).
    pub fn is_playable_leaf(&self) -> bool {
        matches!(
            self,
            Self::Movie | Self::Episode | Self::Track | Self::TvChannel
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
            sdks::stremio::MediaType::Other(s) => match s.as_str() {
                "episode" => Ok(MediaKind::Episode),
                "season" => Ok(MediaKind::Season),
                "person" => Ok(MediaKind::Person),
                _ => Err(()),
            },
        }
    }
}

/// Extracts the addon's raw Stremio type string when it's a non-standard type
/// (e.g. "anime") that `MediaKind` cannot represent and would otherwise collapse
/// to a generic `Movie`/`Series`. Structural keywords (`episode`/`season`/`person`)
/// are not content types and are excluded.
fn custom_stremio_type(media_type: &sdks::stremio::MediaType) -> Option<String> {
    match media_type {
        sdks::stremio::MediaType::Other(s)
            if !matches!(s.as_str(), "episode" | "season" | "person") =>
        {
            Some(s.clone())
        }
        _ => None,
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
    Copy,
    PartialEq,
    Eq,
    Hash,
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

    pub async fn get_by_left_ids(db: &SqlitePool, ids: &[Uuid]) -> Result<Vec<Self>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for chunk in ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT * FROM media_relations WHERE left_media_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            let rows = qb
                .build_query_as::<Self>()
                .fetch_all(db)
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    pub async fn delete_by_ids(db: &SqlitePool, ids: &[Uuid]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        for chunk in ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media_relations WHERE relation_id IN (",
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIds {
    pub imdb: Option<NonEmptyString>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub kitsu: Option<i64>,
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
    /// The addon's own non-standard Stremio type string (e.g. "anime"). The
    /// addon's `/meta/{type}/{id}.json` and `/stream/{type}/{id}.json` routes
    /// require this exact string — losing it causes later lookups to 404.
    pub custom_stremio_type: Option<String>,
    /// Flat album name for tracks that have no parent row (e.g. playlist imports).
    pub album_title: Option<String>,
    /// Flat artist name for tracks that have no grandparent row (e.g. playlist imports).
    pub artist_name: Option<String>,
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
        if let Some(rest) = id.strip_prefix("kitsu:") {
            if let Ok(n) = rest.parse::<i64>() {
                return Self {
                    kitsu: Some(n),
                    // custom_stremio_id drives UUID derivation and the custom-ID
                    // pipeline in stremio_meta_to_medias; keep it set so kitsu items
                    // without an IMDB ID get a stable, deduplicated UUID.
                    custom_stremio_id: Some(id.to_string()),
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
                .tmdb
                .is_none()
            && self
                .tvdb
                .is_none()
            && self
                .custom_stremio_id
                .is_none()
    }

    /// Returns the best Stremio ID for use as a lookup key or idPrefix match.
    /// Priority: imdb → custom_stremio_id → tmdb:{n}
    pub fn stremio_lookup_id(&self) -> Option<String> {
        self.imdb
            .as_deref()
            .map(|s| s.to_string())
            .or_else(|| {
                self.custom_stremio_id
                    .clone()
            })
            .or_else(|| {
                self.tmdb
                    .map(|n| format!("tmdb:{}", n))
            })
    }

    /// All Stremio-formatted ID strings this item could be requested under, in preference
    /// order. For Season/Episode, `grandparent_ext` should be the series' `external_ids`
    /// (from `media.grandparent`); grandparent-derived IDs are omitted when it is absent.
    /// Episodes may still return their own `custom_stremio_id` without a grandparent.
    /// Returns empty when the required `season`/`episode` index is missing.
    ///
    /// `season` = the season index (Season's own `idx`; Episode's `parent_idx`).
    /// `episode` = the episode index (Episode's `idx`); ignored for other kinds.
    pub fn candidate_ids(
        &self,
        kind: &MediaKind,
        season: Option<i64>,
        episode: Option<i64>,
        grandparent_ext: Option<&ExternalIds>,
    ) -> Vec<String> {
        match kind {
            MediaKind::Movie | MediaKind::Series | MediaKind::TvProgram => {
                let mut ids = Vec::new();
                if let Some(ref imdb) = self.imdb {
                    ids.push(imdb.to_string());
                }
                if let Some(ref cid) = self.custom_stremio_id {
                    ids.push(cid.clone());
                }
                if let Some(tmdb) = self.tmdb {
                    ids.push(format!("tmdb:{tmdb}"));
                }
                if let Some(tvdb) = self.tvdb {
                    ids.push(format!("tvdb:{tvdb}"));
                }
                if let Some(kitsu) = self.kitsu {
                    ids.push(format!("kitsu:{kitsu}"));
                }
                ids
            }
            MediaKind::Season => {
                let Some(s) = season else {
                    return Vec::new();
                };
                let gp_imdb = grandparent_ext.and_then(|gp| {
                    gp.imdb
                        .as_deref()
                });
                let gp_custom = grandparent_ext.and_then(|gp| {
                    gp.custom_stremio_id
                        .as_deref()
                });
                let gp_tmdb = grandparent_ext.and_then(|gp| gp.tmdb);
                let gp_tvdb = grandparent_ext.and_then(|gp| gp.tvdb);
                let gp_kitsu = grandparent_ext.and_then(|gp| gp.kitsu);
                let mut ids = Vec::new();
                if let Some(imdb) = gp_imdb {
                    ids.push(format!("{imdb}:{s}"));
                }
                if let Some(cid) = gp_custom {
                    ids.push(format!("{cid}:{s}"));
                }
                if let Some(tmdb) = gp_tmdb {
                    ids.push(format!("tmdb:{tmdb}:{s}"));
                }
                if let Some(tvdb) = gp_tvdb {
                    ids.push(format!("tvdb:{tvdb}:{s}"));
                }
                if let Some(kitsu) = gp_kitsu {
                    ids.push(format!("kitsu:{kitsu}:{s}"));
                }
                ids
            }
            MediaKind::Episode => {
                let (Some(s), Some(e)) = (season, episode) else {
                    return Vec::new();
                };
                let gp_imdb = grandparent_ext.and_then(|gp| {
                    gp.imdb
                        .as_deref()
                });
                let gp_custom = grandparent_ext.and_then(|gp| {
                    gp.custom_stremio_id
                        .as_deref()
                });
                let gp_tmdb = grandparent_ext.and_then(|gp| gp.tmdb);
                let gp_tvdb = grandparent_ext.and_then(|gp| gp.tvdb);
                let gp_kitsu = grandparent_ext.and_then(|gp| gp.kitsu);
                let mut ids = Vec::new();
                // Episode-specific video ID from the addon takes priority.
                if let Some(ref cid) = self.custom_stremio_id {
                    ids.push(cid.clone());
                }
                if let Some(imdb) = gp_imdb {
                    ids.push(format!("{imdb}:{s}:{e}"));
                }
                if let Some(cid) = gp_custom {
                    ids.push(format!("{cid}:{s}:{e}"));
                }
                if let Some(tmdb) = gp_tmdb {
                    ids.push(format!("tmdb:{tmdb}:{s}:{e}"));
                }
                if let Some(tvdb) = gp_tvdb {
                    ids.push(format!("tvdb:{tvdb}:{s}:{e}"));
                }
                if let Some(kitsu) = gp_kitsu {
                    ids.push(format!("kitsu:{kitsu}:{s}:{e}"));
                }
                ids
            }
            MediaKind::Artist => self
                .deezer_artist
                .map(|n| vec![format!("deezer:{n}")])
                .unwrap_or_default(),
            MediaKind::Album => self
                .deezer_album
                .map(|n| vec![format!("deezer:{n}")])
                .unwrap_or_default(),
            MediaKind::Track => self
                .deezer_track
                .map(|n| vec![format!("deezer:{n}")])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn merge(&mut self, source: &Self, replace: bool) {
        use remux_utils::merge_option;
        merge_option(&mut self.imdb, &source.imdb, replace);
        merge_option(&mut self.tmdb, &source.tmdb, replace);
        merge_option(&mut self.tvdb, &source.tvdb, replace);
        merge_option(&mut self.kitsu, &source.kitsu, replace);
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
            &mut self.custom_stremio_type,
            &source.custom_stremio_type,
            replace,
        );
    }

    /// The Stremio `MediaType` to use when querying the source addon: the
    /// captured `custom_stremio_type` when present, otherwise the generic
    /// type derived from `kind`.
    pub fn stremio_media_type(&self, kind: &MediaKind) -> sdks::stremio::MediaType {
        self.custom_stremio_type
            .clone()
            .map(sdks::stremio::MediaType::Other)
            .unwrap_or_else(|| sdks::stremio::MediaType::from(kind))
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
    /// If set, only return items whose `released_at` is on or after this date.
    /// Used by /shows/upcoming to surface future-airing episodes.
    pub released_after: Option<NaiveDateTime>,
    /// Sort order for results. Mapped from Jellyfin's ItemSortBy.
    pub sort_by: Vec<api::ItemSortBy>,
    pub sort_order: Vec<api::SortOrder>,
    /// For TvProgram queries: order by the parent channel's sort_order / channel_number.
    pub sort_by_channel_order: bool,
    /// Restrict Album rows to these release kinds (e.g. only real albums for the
    /// Albums view). `None` = no restriction; albums without a stored kind are
    /// always included.
    pub album_kinds: Option<Vec<AlbumKind>>,
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
    /// Filter by multiple grandparent IDs (OR). Used e.g. for upcoming episodes scoped to a collection's series.
    pub grandparent_ids: Option<Vec<Uuid>>,
    /// Pre-fetched parent item. When set, `get_by_filter` uses it to detect
    /// manual collections and switches to a JOIN on media_relations.
    /// If `parent_id` is set but this is `None`, the non-JOIN path is used.
    pub parent: Option<Media>,
    /// Restrict Genre records to those related (via media_relations) to items
    /// of these content kinds. Used for smart-collection genre queries where
    /// items float freely and cannot be scoped via parent_id / CTE.
    pub genre_related_kinds: Option<Vec<MediaKind>>,
    /// When true, exclude collections/folders/playlists whose `child_count` is 0
    /// after child-count computation. Structural containers whose
    /// `collection_media_kind` is `Collection` (i.e. the "Collections" index
    /// container) are always kept regardless of their count. All other
    /// container kinds — including smart and catalog collections — are dropped
    /// when empty.
    pub exclude_childless: bool,
    pub exclude_ids: Option<Vec<Uuid>>,
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
    /// Series end date (when the show concluded). Derived from `release_info` or episode dates.
    pub end_date: Option<NaiveDateTime>,
    pub album_kind: Option<AlbumKind>,
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

    // --- playlist ownership ---
    /// User that created this playlist. `None` for non-playlist media.
    pub user_id: Option<Uuid>,
    /// When true, the playlist is visible to all authenticated users.
    #[sqlx(default)]
    pub public: bool,

    // --- field locking ---
    /// When true, no metadata provider may overwrite any field on this item.
    #[sqlx(default)]
    pub is_locked: bool,
    /// Per-field locks; a provider skip a field if it appears here.
    #[sqlx(default)]
    #[sqlx(json)]
    pub locked_fields: Vec<MetadataField>,
}

impl Media {
    pub fn is_group_container(&self) -> bool {
        self.kind == MediaKind::Collection
            && self.collection_media_kind == Some(CollectionMediaKind::Collection)
    }

    pub fn is_field_locked(&self, field: &MetadataField) -> bool {
        self.is_locked
            || self
                .locked_fields
                .contains(field)
    }

    /// Best-effort artist name for a music item: the loaded grandparent row
    /// (`self.grandparent`, set by [`Self::preload_parents`]), then the flat
    /// `external_ids.artist_name` (playlist imports have no artist row), then
    /// the legacy `"by {artist}"` description convention.
    pub fn artist_name(&self) -> Option<&str> {
        self.artist_name_from(
            self.grandparent
                .as_deref()
                .map(|g| {
                    g.title
                        .as_str()
                }),
        )
    }

    /// Best-effort album name for a music item: the loaded parent row
    /// (`self.parent`, set by [`Self::preload_parents`]), then the flat
    /// `external_ids.album_title` (playlist imports have no album row).
    pub fn album_name(&self) -> Option<&str> {
        self.album_name_from(
            self.parent
                .as_deref()
                .map(|p| {
                    p.title
                        .as_str()
                }),
        )
    }

    /// Shared artist-name chain. `parent_title` is the title of the artist row
    /// when one is available; callers that resolve it outside `self.grandparent`
    /// (eclipse fetches the row itself, the lyrics API batch-loads several)
    /// pass it here instead.
    pub(crate) fn artist_name_from<'a>(
        &'a self,
        parent_title: Option<&'a str>,
    ) -> Option<&'a str> {
        parent_title
            .filter(|t| !t.is_empty())
            .or_else(|| {
                self.external_ids
                    .artist_name
                    .as_deref()
            })
            .or_else(|| {
                self.description
                    .as_deref()
                    .and_then(|d| d.strip_prefix("by "))
            })
            .filter(|t| !t.is_empty())
    }

    /// Shared album-name chain; `parent_title` is the loaded album row title.
    pub(crate) fn album_name_from<'a>(
        &'a self,
        parent_title: Option<&'a str>,
    ) -> Option<&'a str> {
        parent_title
            .filter(|t| !t.is_empty())
            .or_else(|| {
                self.external_ids
                    .album_title
                    .as_deref()
            })
            .filter(|t| !t.is_empty())
    }

    /// Canonical "Artist Title" search query for track lookups; falls back to
    /// the bare title when no artist is known. Requires parents preloaded via
    /// [`Self::preload_parents`] (or the flat fallback names on the track).
    pub fn track_search_query(&self) -> String {
        self.track_search_query_from(
            self.grandparent
                .as_deref()
                .map(|g| {
                    g.title
                        .as_str()
                }),
        )
    }

    /// Same as [`Self::track_search_query`] but takes the artist row title when
    /// the caller resolved it externally (eclipse fetches the row itself).
    pub fn track_search_query_from(&self, artist_title: Option<&str>) -> String {
        match self.artist_name_from(artist_title) {
            Some(artist) => format!("{} {}", artist, self.title),
            None => self
                .title
                .clone(),
        }
    }

    /// Deezer search query that pins the artist and kind so a title-only match
    /// can't resolve to the wrong artist's track/album.
    pub fn deezer_search_query(&self, kind: &str) -> String {
        match self.artist_name() {
            Some(artist) => format!(
                "artist:\"{}\" {kind}:\"{}\"",
                artist.replace('"', ""),
                self.title
                    .replace('"', ""),
            ),
            None => self
                .title
                .clone(),
        }
    }

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
            kind: MediaKind,
            channel_number: Option<i64>,
            external_ids: ExternalIds,
        }

        let mut parent_map: HashMap<Uuid, ParentRow> = HashMap::new();
        for chunk in ids_needed.chunks(500) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT id, title, kind, channel_number, external_ids FROM media WHERE id IN (",
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
                            let kind: Option<MediaKind> = r.get(2);
                            let channel_number: Option<i64> = r.get(3);
                            let external_ids: ExternalIds = r
                                .try_get::<Option<String>, _>(4)
                                .ok()
                                .flatten()
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_default();
                            id.zip(title)
                                .zip(kind)
                                .map(|((id, title), kind)| {
                                    (
                                        id,
                                        ParentRow {
                                            id,
                                            title,
                                            kind,
                                            channel_number,
                                            external_ids,
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
                m.kind = row
                    .kind
                    .clone();
                m.channel_number = row.channel_number;
                m.external_ids = row
                    .external_ids
                    .clone();
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

    /// Fill `runtime` for Playlist rows from the summed runtime of their
    /// member items, and for Album/Artist rows from the summed runtime of
    /// their child tracks. Jellyfin reports playlist/album/artist duration
    /// (`RunTimeTicks`) computed from child items; Remux stores no runtime of
    /// its own on those rows, so clients like Feishin end up with NaN when the
    /// field is absent from the serialized DTO ("0 seconds" headers).
    pub async fn preload_playlist_runtimes(db: &SqlitePool, records: &mut [Self]) {
        let playlist_ids: Vec<Uuid> = records
            .iter()
            .filter(|m| {
                m.kind == MediaKind::Playlist
                    && m.runtime
                        .is_none()
            })
            .map(|m| m.id)
            .collect();
        let album_ids: Vec<Uuid> = records
            .iter()
            .filter(|m| {
                m.kind == MediaKind::Album
                    && m.runtime
                        .is_none()
            })
            .map(|m| m.id)
            .collect();
        let artist_ids: Vec<Uuid> = records
            .iter()
            .filter(|m| {
                m.kind == MediaKind::Artist
                    && m.runtime
                        .is_none()
            })
            .map(|m| m.id)
            .collect();
        if playlist_ids.is_empty() && album_ids.is_empty() && artist_ids.is_empty() {
            return;
        }

        let mut runtime_map: std::collections::HashMap<Uuid, i64> = Default::default();

        for chunk in playlist_ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT mr.left_media_id, SUM(m.runtime) FROM media_relations mr JOIN media m ON m.id = mr.right_media_id WHERE mr.role = 'playlist' AND mr.left_media_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(") GROUP BY mr.left_media_id");

            match qb
                .build()
                .fetch_all(db)
                .await
            {
                Ok(rows) => {
                    for row in rows {
                        let pid: Uuid = row.get(0);
                        let total: Option<i64> = row.get(1);
                        if let Some(runtime) = total {
                            runtime_map.insert(pid, runtime);
                        }
                    }
                }
                Err(e) => {
                    warn!("failed to preload playlist runtimes: {e}");
                }
            }
        }

        for chunk in album_ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT parent_id, SUM(runtime) FROM media WHERE kind = 'track' AND parent_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(") GROUP BY parent_id");

            match qb
                .build()
                .fetch_all(db)
                .await
            {
                Ok(rows) => {
                    for row in rows {
                        let pid: Uuid = row.get(0);
                        let total: Option<i64> = row.get(1);
                        if let Some(runtime) = total {
                            runtime_map.insert(pid, runtime);
                        }
                    }
                }
                Err(e) => {
                    warn!("failed to preload album runtimes: {e}");
                }
            }
        }

        for chunk in artist_ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT grandparent_id, SUM(runtime) FROM media WHERE kind = 'track' AND grandparent_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(") GROUP BY grandparent_id");

            match qb
                .build()
                .fetch_all(db)
                .await
            {
                Ok(rows) => {
                    for row in rows {
                        let pid: Uuid = row.get(0);
                        let total: Option<i64> = row.get(1);
                        if let Some(runtime) = total {
                            runtime_map.insert(pid, runtime);
                        }
                    }
                }
                Err(e) => {
                    warn!("failed to preload artist runtimes: {e}");
                }
            }
        }

        for media in records.iter_mut() {
            if let Some(runtime) = runtime_map.get(&media.id) {
                media.runtime = Some(*runtime);
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
    pub fn is_live(&self) -> bool {
        self.kind == MediaKind::TvChannel
    }

    pub fn is_track(&self) -> bool {
        self.kind == MediaKind::Track
    }

    pub fn full_title(&self) -> String {
        match self.kind {
            MediaKind::Episode | MediaKind::TvProgram => {
                let show = self
                    .grandparent
                    .as_deref()
                    .map(|g| {
                        g.title
                            .as_str()
                    })
                    .unwrap_or_default();
                match (show.is_empty(), self.parent_idx, self.idx) {
                    (false, Some(s), Some(e)) => {
                        format!("{} S{:02}E{:02} - {}", show, s, e, self.title)
                    }
                    (false, _, _) => format!("{} - {}", show, self.title),
                    (true, Some(s), Some(e)) => {
                        format!("S{:02}E{:02} - {}", s, e, self.title)
                    }
                    _ => self
                        .title
                        .clone(),
                }
            }
            MediaKind::Track => {
                let artist = self
                    .artist_name()
                    .unwrap_or_default();
                if artist.is_empty() {
                    self.title
                        .clone()
                } else {
                    format!("{} - {}", artist, self.title)
                }
            }
            _ => self
                .title
                .clone(),
        }
    }

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

    /// All Stremio-formatted IDs this item could be requested under. Convenience wrapper
    /// over `ExternalIds::candidate_ids` that maps Season/Episode index fields correctly.
    pub fn candidate_ids(&self, grandparent_ext: Option<&ExternalIds>) -> Vec<String> {
        let season = match self.kind {
            MediaKind::Season => self.idx,
            MediaKind::Episode => self.parent_idx,
            _ => None,
        };
        let episode = if self.kind == MediaKind::Episode {
            self.idx
        } else {
            None
        };
        self.external_ids
            .candidate_ids(&self.kind, season, episode, grandparent_ext)
    }

    /// Returns the grandparent `Media`, loading and caching it from the DB if not already set.
    /// Returns `None` when this item has no `grandparent_id`.
    pub async fn grandparent(
        &mut self,
        db: &SqlitePool,
    ) -> Result<Option<&Self>, sqlx::Error> {
        if self
            .grandparent
            .is_none()
        {
            if let Some(gp_id) = self.grandparent_id {
                if let Some(gp) = Self::get_by_id(db, &gp_id).await? {
                    self.grandparent = Some(Box::new(gp));
                }
            }
        }
        Ok(self
            .grandparent
            .as_deref())
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
            MediaKind::Season | MediaKind::Episode => self
                .grandparent_id
                .is_none()
                .then_some("grandparent_id"),
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

        if matches!(self.kind, MediaKind::Movie | MediaKind::Series) {
            let raw = self.media_id_raw();
            if raw
                .canonical()
                .is_none()
            {
                return Err(MediaError::ValidationError(format!(
                    "{:?} '{}' has no canonical external ID — cannot assign stable UUID",
                    self.kind, self.title,
                )));
            }
            let expected = Uuid::from(&raw);
            if expected != self.id && !Self::ext_id_uuid_all(self).contains(&self.id) {
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
            collection_default_sort, collection_default_sort_order,
            original_language, is_locked, locked_fields, album_kind, end_date,
            user_id, public
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41, $42, $43, $44, $45, $46, $47, $48, $49)
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
            -- Widened, not replaced: a write that resolved no ids of its own
            -- omits them (`skip_serializing_none`), and must not drop ids
            -- another one has since resolved. See `widen_external_ids`.
            external_ids = json_patch(
                CASE WHEN json_valid(media.external_ids)
                     THEN media.external_ids ELSE '{}' END,
                excluded.external_ids
            ),
            external_ratings = COALESCE(excluded.external_ratings, media.external_ratings),
            promoted = excluded.promoted,
            collection_kind = excluded.collection_kind,
            collection_media_kind = excluded.collection_media_kind,
            collection_max_items = excluded.collection_max_items,
            collection_smart_filter = excluded.collection_smart_filter,
            collection_latest_auto_unplayed = excluded.collection_latest_auto_unplayed,
            collection_latest_sort_digital = excluded.collection_latest_sort_digital,
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
            original_language = COALESCE(excluded.original_language, media.original_language),
            is_locked = excluded.is_locked,
            locked_fields = excluded.locked_fields,
            album_kind = COALESCE(excluded.album_kind, media.album_kind),
            end_date = COALESCE(excluded.end_date, media.end_date),
            user_id = COALESCE(excluded.user_id, media.user_id),
            public = excluded.public
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
        .bind(sqlx::types::Json(&self.collection_default_sort))
        .bind(sqlx::types::Json(&self.collection_default_sort_order))
        .bind(&self.original_language)
        .bind(self.is_locked)
        .bind(sqlx::types::Json(&self.locked_fields))
        .bind(&self.album_kind)
        .bind(self.end_date)
        .bind(self.user_id)
        .bind(self.public)
        .execute(db)
        .await?;

        MediaImage::sync_from_media(db, self.id, &self.images)
            .await
            .ok();

        Ok(())
    }

    /// Widen `external_ids` with `patch`, without touching any other column.
    ///
    /// Merged in SQL, not from the caller's snapshot: two lookups enriching one
    /// item would otherwise fill different ids and the later write erase the
    /// earlier. Stored ids win, as in [`ExternalIds::merge`] with
    /// `replace: false`, so enrichment only ever adds. The stored side is the
    /// merge patch here, so its nulls are dropped first: a null in a patch
    /// deletes the key rather than leaving it, which would erase the very id
    /// being added. Invalid JSON is treated as empty. Either repairs the row
    /// rather than failing the write.
    pub async fn widen_external_ids(
        db: &sqlx::SqlitePool,
        id: &Uuid,
        patch: &ExternalIds,
    ) -> Result<Option<ExternalIds>> {
        let merged: Option<sqlx::types::Json<ExternalIds>> = sqlx::query_scalar(
            "UPDATE media \
             SET external_ids = json_patch( \
                     ?1, \
                     json_patch('{}', CASE WHEN json_valid(external_ids) \
                                           THEN external_ids ELSE '{}' END) \
                 ), \
                 updated_at = ?2 \
             WHERE id = ?3 \
             RETURNING external_ids",
        )
        .bind(sqlx::types::Json(patch))
        .bind(Utc::now().naive_utc())
        .bind(id)
        .fetch_optional(db)
        .await?;
        Ok(merged.map(|j| j.0))
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
                collection_default_sort, collection_default_sort_order,
                original_language, is_locked, locked_fields, album_kind, end_date
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
                    .push_bind(sqlx::types::Json(&item.collection_default_sort))
                    .push_bind(sqlx::types::Json(&item.collection_default_sort_order))
                    .push_bind(&item.original_language)
                    .push_bind(&item.is_locked)
                    .push_bind(sqlx::types::Json(&item.locked_fields))
                    .push_bind(&item.album_kind)
                    .push_bind(&item.end_date);
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

        let mut items: Vec<Self> = items
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
                collection_default_sort, collection_default_sort_order,
                original_language, is_locked, locked_fields, album_kind, end_date
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
                    .push_bind(sqlx::types::Json(&item.collection_default_sort))
                    .push_bind(sqlx::types::Json(&item.collection_default_sort_order))
                    .push_bind(&item.original_language)
                    .push_bind(&item.is_locked)
                    .push_bind(sqlx::types::Json(&item.locked_fields))
                    .push_bind(&item.album_kind)
                    .push_bind(&item.end_date);
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
                external_ids = json_patch(
                    CASE WHEN json_valid(media.external_ids)
                         THEN media.external_ids ELSE '{}' END,
                    excluded.external_ids
                ),
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
                original_language = COALESCE(excluded.original_language, media.original_language),
                -- preserve user-set locks; never let a provider refresh overwrite them
                is_locked = CASE WHEN media.id IS NOT NULL THEN media.is_locked ELSE excluded.is_locked END,
                locked_fields = CASE WHEN media.id IS NOT NULL THEN media.locked_fields ELSE excluded.locked_fields END,
                album_kind = COALESCE(excluded.album_kind, media.album_kind),
                end_date = COALESCE(excluded.end_date, media.end_date)",
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

    /// Look up an existing DB row that shares any external ID with `self`.
    /// Returns the existing row's UUID so the caller can adopt it before upserting,
    /// preventing duplicate rows when the same content arrives with different canonical IDs.
    ///
    /// Only called for root-level items (Movie, Series, Artist, Album, Track).
    /// Season / Episode deduplication uses `(parent_id, kind, idx)` instead.
    pub async fn find_existing_id_by_ext(db: &SqlitePool, item: &Self) -> Option<Uuid> {
        let candidates = Self::ext_id_uuid_candidates(item);
        if candidates.is_empty() {
            return None;
        }
        let placeholders = candidates
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT id FROM media WHERE id IN ({placeholders}) LIMIT 1");
        let mut q = sqlx::query_scalar::<_, Uuid>(&sql);
        for uuid in &candidates {
            q = q.bind(*uuid);
        }
        q.fetch_optional(db)
            .await
            .ok()
            .flatten()
    }

    /// Stable anchor key for season/episode UUID derivation: the series'
    /// canonical external-ID string (imdb ▸ custom ▸ tmdb ▸ tvdb ▸ kitsu,
    /// mirroring `MediaIdRaw::canonical`). Falls back to the series' own UUID
    /// when nothing external is resolvable.
    pub fn series_canonical_key(&self) -> String {
        Self::series_canonical_key_ext(&self.external_ids).unwrap_or_else(|| {
            self.id
                .to_string()
        })
    }

    /// The canonical external-ID key for a series: the first entry from `candidate_ids`.
    pub fn series_canonical_key_ext(ext: &ExternalIds) -> Option<String> {
        ext.candidate_ids(&MediaKind::Series, None, None, None)
            .into_iter()
            .next()
    }

    /// Single source of truth for season UUIDs in the canonical (flat) scheme:
    /// `stable_media_uuid(Season, "{series_key}:{season_idx}")`.
    pub fn season_id(series_key: &str, season_idx: i64) -> Uuid {
        crate::common::stable_media_uuid(
            &MediaKind::Season,
            &format!("{series_key}:{season_idx}"),
        )
    }

    /// Single source of truth for episode UUIDs in the canonical (flat) scheme:
    /// `stable_media_uuid(Episode, "{series_key}:{season_idx}:{ep_idx}")`.
    pub fn episode_id(series_key: &str, season_idx: i64, ep_idx: i64) -> Uuid {
        crate::common::stable_media_uuid(
            &MediaKind::Episode,
            &format!("{series_key}:{season_idx}:{ep_idx}"),
        )
    }

    /// Legacy parent-anchored season UUID (pre-flattening scheme):
    /// `stable_media_uuid(Season, "{series_uuid}:{season_idx}")`. Only used to
    /// build migration candidates for rows stored under the old scheme.
    pub fn season_id_nested(series_uuid: Uuid, season_idx: i64) -> Uuid {
        crate::common::stable_media_uuid(
            &MediaKind::Season,
            &format!("{series_uuid}:{season_idx}"),
        )
    }

    /// Legacy parent-anchored episode UUID (pre-flattening scheme):
    /// `stable_media_uuid(Episode, "{season_uuid}:{ep_idx}")`. Only used to
    /// build migration candidates for rows stored under the old scheme.
    pub fn episode_id_nested(season_uuid: Uuid, ep_idx: i64) -> Uuid {
        crate::common::stable_media_uuid(
            &MediaKind::Episode,
            &format!("{season_uuid}:{ep_idx}"),
        )
    }

    /// Compute all candidate UUIDs an existing DB row could have been stored under
    /// for the given item's external IDs. Used by `find_existing_id_by_ext` (dedup)
    /// and `UserMediaState::get_or_new` (legacy state lookup).
    ///
    /// Each external ID is turned into the stable UUID it would produce if it were
    /// the canonical key at insert time. Seasons/Episodes additionally get the
    /// parent-anchored UUIDs the Stremio importer writes (recomputed from
    /// `grandparent_id` + indices) plus flat keys from the grandparent series'
    /// external IDs. The item's own current UUID is excluded so only *different*
    /// rows can match.
    pub fn ext_id_uuid_candidates(item: &Self) -> Vec<Uuid> {
        let mut candidates = Self::ext_id_uuid_all(item);
        candidates.retain(|u| *u != item.id);
        candidates
    }

    /// As `ext_id_uuid_candidates`, but keeps the row's current UUID. `validate`
    /// needs the unfiltered set: a row whose canonical ID changed after it was
    /// written (Kitsu anime that later gains an IMDB ID) still owns the UUID it
    /// was stored under.
    fn ext_id_uuid_all(item: &Self) -> Vec<Uuid> {
        use crate::common::stable_media_uuid;
        let kind = &item.kind;
        let ext = &item.external_ids;
        let mut candidates: Vec<Uuid> = Vec::new();
        match kind {
            MediaKind::Movie | MediaKind::Series | MediaKind::TvProgram => {
                if let Some(imdb) = ext
                    .imdb
                    .as_deref()
                {
                    candidates.push(stable_media_uuid(kind, imdb));
                }
                if let Some(custom) = ext
                    .custom_stremio_id
                    .as_deref()
                {
                    candidates.push(stable_media_uuid(kind, custom));
                }
                if let Some(tmdb) = ext.tmdb {
                    candidates.push(stable_media_uuid(kind, &format!("tmdb:{tmdb}")));
                }
                if let Some(tvdb) = ext.tvdb {
                    candidates.push(stable_media_uuid(kind, &format!("tvdb:{tvdb}")));
                }
                if let Some(kitsu) = ext.kitsu {
                    candidates.push(stable_media_uuid(kind, &format!("kitsu:{kitsu}")));
                }
            }
            MediaKind::Season | MediaKind::Episode => {
                // Season/Episode UUIDs are not derived from a single external ID:
                // the Stremio path anchors them to the series UUID
                // (season = f(series_id, season_idx), episode = f(season_id, ep_idx)),
                // while other paths use flat keys like f(imdb, season, episode).
                // After a library purge + repopulate the anchors can change, so we
                // emit every UUID the row could plausibly be stored under:
                //
                // 1. the parent/nested scheme the Stremio importer writes
                // 2. flat keys derived from the grandparent series' external IDs
                // 3. the episode's own raw Stremio ID (used as a candidate upstream)
                let (season_idx, episode_idx) = match kind {
                    MediaKind::Season => (item.idx, None),
                    _ => (item.parent_idx, item.idx),
                };
                let Some(s) = season_idx else {
                    return candidates;
                };

                // (1) nested: season_uuid = stable(Season, "{series_id}:{s}")
                let series_id = item
                    .grandparent_id
                    .or(item.parent_id);
                if let Some(series_id) = series_id {
                    let season_uuid = Self::season_id_nested(series_id, s);
                    match kind {
                        MediaKind::Season => candidates.push(season_uuid),
                        _ => {
                            if let Some(e) = episode_idx {
                                candidates
                                    .push(Self::episode_id_nested(season_uuid, e));
                            }
                        }
                    }
                }

                // (2) flat keys derived from the same candidate_ids() the addon/
                // route matching uses: every (series external ID, season[, episode])
                // combination, plus the episode's own Stremio ID.
                let gp_ext = item
                    .grandparent
                    .as_deref()
                    .map(|gp| &gp.external_ids);
                for id_str in item.candidate_ids(gp_ext) {
                    candidates.push(stable_media_uuid(kind, &id_str));
                }

                // (3) the episode's own Stremio ID is already covered by
                // candidate_ids() above.
            }
            MediaKind::Artist => {
                if let Some(id) = ext.deezer_artist {
                    candidates.push(stable_media_uuid(kind, &id.to_string()));
                }
            }
            MediaKind::Album => {
                if let Some(id) = ext.deezer_album {
                    candidates.push(stable_media_uuid(kind, &id.to_string()));
                }
            }
            MediaKind::Track => {
                if let Some(id) = ext.deezer_track {
                    candidates.push(stable_media_uuid(kind, &id.to_string()));
                }
            }
            _ => {}
        }
        candidates
    }

    /// Update all `parent_id` / `grandparent_id` references from `old_id` to `new_id`
    /// and migrate any `user_media_state` rows. Used when a root UUID is adopted from DB.
    pub async fn cascade_update_parent_refs(
        db: &SqlitePool,
        old_id: Uuid,
        new_id: Uuid,
    ) -> Result<()> {
        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        let mut tx = db
            .begin()
            .await?;
        sqlx::query("UPDATE media SET parent_id = ? WHERE parent_id = ?")
            .bind(new_id)
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE media SET grandparent_id = ? WHERE grandparent_id = ?")
            .bind(new_id)
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE user_media_state SET media_id = ? WHERE media_id = ?")
            .bind(new_id)
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE media_relations SET left_media_id = ? WHERE left_media_id = ?",
        )
        .bind(new_id)
        .bind(old_id)
        .execute(&mut *tx)
        .await?;
        tx.commit()
            .await?;
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

        // Drive from media_relations using idx_media_relations_right_left so we
        // only visit rows that share one of the target genres, then join to media
        // by primary key to filter by kind. genre_ids are already filtered to
        // genre/music_genre kinds by the query above, so no JOIN back to media g
        // is needed.
        let base = "SELECT mr.left_media_id as id, COUNT(DISTINCT mr.right_media_id) as score \
                    FROM media_relations mr \
                    JOIN media m ON m.id = mr.left_media_id AND m.kind = ";

        // Count total.
        let mut count_qb =
            sqlx::QueryBuilder::new(format!("SELECT COUNT(*) FROM ({} ", base));
        count_qb.push_bind(&kind_str);
        count_qb.push(" AND m.id != ");
        count_qb.push_bind(source_id);
        count_qb.push(" WHERE mr.right_media_id IN (");
        let mut sep = count_qb.separated(", ");
        for gid in &genre_ids {
            sep.push_bind(*gid);
        }
        count_qb.push(") GROUP BY mr.left_media_id) sub");
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(db)
            .await?;

        // Fetch scored page.
        let mut qb = sqlx::QueryBuilder::new(base);
        qb.push_bind(&kind_str);
        qb.push(" AND m.id != ");
        qb.push_bind(source_id);
        qb.push(" WHERE mr.right_media_id IN (");
        let mut sep = qb.separated(", ");
        for gid in &genre_ids {
            sep.push_bind(*gid);
        }
        qb.push(") GROUP BY mr.left_media_id ORDER BY score DESC LIMIT ");
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

    pub async fn set_parent_id(
        db: &SqlitePool,
        media_ids: &[Uuid],
        parent_id: Option<Uuid>,
    ) -> Result<(), sqlx::Error> {
        if media_ids.is_empty() {
            return Ok(());
        }
        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        for chunk in media_ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new("UPDATE media SET parent_id = ");
            qb.push_bind(parent_id);
            qb.push(" WHERE id IN (");
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

    /// Like `set_parent_id(None, ...)` but only clears rows whose `parent_id`
    /// currently equals `required_parent_id`. Prevents accidentally detaching
    /// items that belong to a different parent.
    pub async fn clear_parent_id_scoped(
        db: &SqlitePool,
        media_ids: &[Uuid],
        required_parent_id: &Uuid,
    ) -> Result<(), sqlx::Error> {
        if media_ids.is_empty() {
            return Ok(());
        }
        let _permit = DB_WRITE_SEMAPHORE
            .acquire()
            .await
            .unwrap();
        for chunk in media_ids.chunks(SQLITE_VAR_LIMIT) {
            let mut qb = sqlx::QueryBuilder::new(
                "UPDATE media SET parent_id = NULL WHERE parent_id = ",
            );
            qb.push_bind(required_parent_id);
            qb.push(" AND id IN (");
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

    /// Fetch media rows by id, chunking the `IN (...)` clause so queries stay
    /// under SQLite's 999-variable limit (SQLITE_VAR_LIMIT).
    pub async fn get_by_ids(db: &SqlitePool, ids: &[Uuid]) -> Result<Vec<Self>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(SQLITE_VAR_LIMIT) {
            out.extend(
                Self::get_by_filter(
                    db,
                    &MediaFilter {
                        id: Some(chunk.to_vec()),
                        ..Default::default()
                    },
                )
                .await?
                .records,
            );
        }
        Ok(out)
    }

    pub async fn get_by_filter(
        db: &SqlitePool,
        filter: &MediaFilter,
    ) -> Result<FilterResult<Media>> {
        use tokio_util::future::FutureExt;
        Self::get_by_filter_inner(db, filter)
            .timeout(std::time::Duration::from_secs(30))
            .await
            .map_err(|_| {
                error!(?filter, "get_by_filter timed out after 30s");
                anyhow!("get_by_filter timed out after 30s: {filter:?}")
            })?
    }

    async fn get_by_filter_inner(
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
            .map(|p| matches!(p.collection_kind, Some(CollectionKind::Smart)))
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

        // Only treat Played=true as a definite requirement when it appears inside an
        // All-mode group within an All-mode top-level filter. If it's in an Any group,
        // the rule is optional (other rules can satisfy the group), so the CTE path —
        // which restricts the result set to only played items — would wrongly exclude
        // non-played items that match the other rules in that group.
        let has_watched_true_rule = filter
            .filter_rules
            .as_ref()
            .map(|cf| {
                cf.match_mode == sdks::remux::FilterMatchMode::All
                    && cf
                        .groups
                        .iter()
                        .any(|g| {
                            g.match_mode == sdks::remux::FilterMatchMode::All
                                && g.rules
                                    .iter()
                                    .any(|r| {
                                        matches!(
                                            r,
                                            sdks::remux::FilterRule::Played {
                                                value: true
                                            }
                                        )
                                    })
                        })
            })
            .unwrap_or(false);

        // When Watched=true filter is active AND DatePlayed sort is requested, inject a
        // watched_dates CTE that computes effective watched dates once (rolling episode
        // plays up to grandparent series). The main query drives FROM watched_dates → media
        // (PK join), skipping all unwatched rows with no per-row subquery. ORDER BY uses
        // wd.effective_date directly. The Watched filter rule itself is suppressed (the
        // CTE JOIN already enforces membership). Without the CTE, the dp driving table
        // can't be used because series with only episode-level plays have no direct UMS row.
        let use_watched_cte = has_watched_true_rule
            && filter
                .user_id
                .is_some()
            && filter
                .sort_by
                .iter()
                .any(|s| matches!(s, api::ItemSortBy::DatePlayed));

        let watched_cte_sql: Option<String> = if use_watched_cte {
            filter.user_id.as_ref().map(|uid| {
                format!(
                    "WITH watched_dates AS (\
                      SELECT COALESCE(ep.grandparent_id, ums.media_id) AS effective_id, \
                             MAX(COALESCE(ums.last_played_at, ums.played_at)) AS effective_date \
                      FROM user_media_state ums \
                      LEFT JOIN media ep ON ep.id = ums.media_id AND ep.kind = 'episode' \
                      WHERE ums.user_id = X'{}' AND ums.play_count > 0 \
                      GROUP BY effective_id\
                    ) ",
                    uid.simple()
                )
            })
        } else {
            None
        };

        // When sorting by DatePlayed, drive records_qb FROM user_media_state (dp) so
        // the result is already in last_played_at order — no correlated subquery per row,
        // no separate sort pass. Disabled when has_watched_true_rule: dp requires a direct
        // UMS row; partially watched series (episode-only plays) have none, so they'd drop
        // from results. The watched path uses the CTE approach above instead.
        let date_played_uid = if has_watched_true_rule {
            None
        } else {
            filter
                .sort_by
                .iter()
                .any(|s| matches!(s, api::ItemSortBy::DatePlayed))
                .then(|| {
                    filter
                        .user_id
                        .as_ref()
                })
                .flatten()
        };

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
            if let Some(ref cte) = watched_cte_sql {
                count_qb = sqlx::QueryBuilder::new(format!(
                    "{cte}SELECT COUNT(*) as count \
                     FROM watched_dates wd JOIN media ON media.id = wd.effective_id WHERE 1=1"
                ));
                records_qb = sqlx::QueryBuilder::new(format!(
                    "{cte}SELECT media.* \
                     FROM watched_dates wd JOIN media ON media.id = wd.effective_id WHERE 1=1"
                ));
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
                } else if pop_period.is_some() {
                    pop_joined = true;
                    // Build a CTE over the media table so the WHERE conditions loop
                    // below can fill it once. After the loop we close the CTE and
                    // wrap it in a UNION ALL: arm 1 drives from idx_pop_agg_covering
                    // (scored items in avg-DESC order via the index walk), arm 2
                    // streams unscored items via NOT EXISTS. SQLite evaluates UNION ALL
                    // arms as coroutines — no global sort, LIMIT stops after arm 1 if
                    // there are enough scored items.
                    records_qb = sqlx::QueryBuilder::new(
                        "WITH filtered AS (SELECT media.* FROM media WHERE 1=1",
                    );
                } else {
                    records_qb =
                        sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");
                }
            } // end else (non-CTE path)
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
                        pre_qb.push(" AND playback_position > 0");
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
                    qb.push(")");
                    if is_genre_scope_query {
                        if let Some(ref rules) = filter.filter_rules {
                            qb.push(
                                " AND item.id IN (SELECT media.id FROM media WHERE 1 = 1",
                            );
                            apply_filter_rules(
                                qb,
                                rules,
                                filter
                                    .user_id
                                    .as_ref(),
                                // Independent subquery: it does not join the
                                // watched CTE, so the played-true rule must be
                                // applied inline rather than suppressed.
                                false,
                            );
                            qb.push(")");
                        }
                    }
                    qb.push(")");
                }
            }
            if let Some(grandparent_id) = &filter.grandparent_id {
                qb.push(" AND grandparent_id = ")
                    .push_bind(grandparent_id);
            }
            if let Some(ids) = &filter.grandparent_ids {
                if !ids.is_empty() {
                    qb.push_in("grandparent_id", ids);
                }
            }
            if let Some(promoted) = &filter.promoted {
                qb.push(" AND promoted = ")
                    .push_bind(promoted);
            }
            if let Some(kind) = &filter.kind {
                if resumable_ids.is_none() {
                    qb.push_in("kind", &kind);
                }
                // Scope playlist visibility: show only public playlists or those owned by the caller.
                let has_playlist = kind
                    .iter()
                    .any(|k| *k == MediaKind::Playlist);
                if has_playlist {
                    if let Some(uid) = filter.user_id {
                        qb.push(" AND (kind != 'playlist' OR public = 1 OR user_id = ");
                        qb.push_bind(uid);
                        qb.push(")");
                    } else {
                        qb.push(" AND (kind != 'playlist' OR public = 1)");
                    }
                }
            }
            if let Some(kinds) = &filter.album_kinds {
                if !kinds.is_empty() {
                    // Only the requested release kinds; albums without a stored
                    // kind (NULL) are treated as albums and always included.
                    qb.push(" AND (album_kind IS NULL OR album_kind IN (");
                    let mut sep = qb.separated(", ");
                    for k in kinds {
                        sep.push_bind(k);
                    }
                    qb.push("))");
                }
            }
            if let Some(id) = &filter.id {
                qb.push_in("id", &id);
            }

            if let Some(genre_ids) = &filter.genre_ids {
                if !genre_ids.is_empty() {
                    // Direct relation (album/artist/movie → genre), plus a
                    // fallback for tracks: music genres are persisted at
                    // album level only, so inherit them from the parent album.
                    qb.push(" AND (EXISTS (SELECT 1 FROM media_relations mr WHERE mr.left_media_id = media.id AND mr.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in genre_ids {
                        sep.push_bind(id);
                    }
                    qb.push(")) OR (media.kind = 'track' AND EXISTS (SELECT 1 FROM media_relations mr2 JOIN media p ON p.id = mr2.left_media_id WHERE mr2.right_media_id IN (");
                    let mut sep = qb.separated(", ");
                    for id in genre_ids {
                        sep.push_bind(id);
                    }
                    qb.push(") AND p.kind = 'album' AND p.id = media.parent_id)))");
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

                // played=true — non-correlated IN. Two branches unioned:
                //   1. Direct: episodes/movies/series marked played themselves.
                //   2. Rollup: series whose episodes have play_count > 0.
                // Replaces a per-row EXISTS(UNION ALL) that was O(series × episodes).
                if user_state_filter.played == Some(true) {
                    qb.push(
                        " AND media.id IN (\
                          SELECT ums.media_id \
                          FROM user_media_state ums \
                          WHERE ums.play_count > 0",
                    );
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ")
                            .push_bind(user_id);
                    }
                    qb.push(
                        " UNION \
                          SELECT ep.grandparent_id \
                          FROM user_media_state ums \
                          JOIN media ep ON ep.id = ums.media_id AND ep.kind = 'episode' \
                          WHERE ums.play_count > 0",
                    );
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ")
                            .push_bind(user_id);
                    }
                    qb.push(" AND ep.grandparent_id IS NOT NULL)");
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
                    // Parent fallback (correlated subquery to parent row) is only
                    // meaningful for episodes that have no own air date and must
                    // inherit the series premiere. For Movie/Series/Track/etc.
                    // parent_id is NULL so the subquery always returns NULL — it's
                    // pure overhead. Enable only when the query may include episodes.
                    let needs_parent_fallback = filter
                        .kind
                        .as_ref()
                        .map(|k| {
                            k.is_empty()
                                || k.iter()
                                    .any(|k| matches!(k, MediaKind::Episode))
                        })
                        .unwrap_or(true);
                    push_release_date_filter(
                        qb,
                        "media",
                        threshold,
                        needs_parent_fallback,
                    );
                }
            }

            if let Some(after) = filter.released_after {
                qb.push(" AND released_at >= ")
                    .push_bind(after);
            }

            if !is_genre_scope_query {
                if let Some(ref f) = filter.filter_rules {
                    // When the query targets child types (episodes/seasons), the
                    // collection filter rules describe which series qualify — not the
                    // episode rows themselves. Apply them via a grandparent subquery so
                    // only children of matching series are returned.
                    let all_episode_or_season = filter
                        .kind
                        .as_ref()
                        .map(|k| {
                            !k.is_empty()
                                && k.iter()
                                    .all(|k| {
                                        matches!(
                                            k,
                                            MediaKind::Episode | MediaKind::Season
                                        )
                                    })
                        })
                        .unwrap_or(false);

                    if all_episode_or_season {
                        if let Some(fragment) = build_filter_rules_fragment(
                            f,
                            filter
                                .user_id
                                .as_ref(),
                            false,
                        ) {
                            qb.push(format!(
                                " AND grandparent_id IN \
                            (SELECT id FROM media WHERE kind = 'series' AND {fragment})"
                            ));
                        }
                    } else {
                        apply_filter_rules(
                            qb,
                            f,
                            filter
                                .user_id
                                .as_ref(),
                            use_watched_cte,
                        );
                    }
                }
            }
            if let Some(ref ids) = filter.exclude_ids {
                if !ids.is_empty() {
                    qb.push(" AND media.id NOT IN (");
                    let mut sep = qb.separated(", ");
                    for id in ids {
                        sep.push_bind(*id);
                    }
                    qb.push(")");
                }
            }
            // Policy filters (rating, tags, smart policy rules) must not apply to:
            // - container rows (Collection/Folder/Playlist) — CLAUDE.md rule
            // - music content (Track/Album/Artist/MusicGenre) — ratings/tags are irrelevant
            // - live TV (TvChannel/TvProgram) — same reason
            // All four policy conditions live here — one place, one guard.
            let container_only = filter
                .kind
                .as_deref()
                .map_or(false, |ks| {
                    !ks.is_empty()
                        && ks
                            .iter()
                            .all(|k| {
                                matches!(
                                    k,
                                    MediaKind::Collection
                                        | MediaKind::Folder
                                        | MediaKind::Playlist
                                )
                            })
                });

            let has_policy = filter
                .max_parental_rating
                .is_some()
                || filter
                    .blocked_tags
                    .as_ref()
                    .map_or(false, |v| !v.is_empty())
                || filter
                    .allowed_tags
                    .as_ref()
                    .map_or(false, |v| !v.is_empty())
                || filter
                    .policy_filter
                    .is_some();

            if !container_only && has_policy {
                if let Some(max_rating) = filter.max_parental_rating {
                    qb.push(" AND COALESCE(certification_age, (SELECT p.certification_age FROM media p WHERE p.id = COALESCE(media.grandparent_id, media.parent_id))) <= ")
                        .push_bind(max_rating)
                        .push("");
                }

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

                if let Some(ref f) = filter.policy_filter {
                    apply_filter_rules(
                        qb,
                        f,
                        filter
                            .user_id
                            .as_ref(),
                        false,
                    );
                }
            }
        }

        // Close the filtered CTE and build the UNION ALL structure.
        // Arm 1 joins popularity_agg → filtered driving from idx_pop_agg_covering,
        // producing scored items in avg-DESC order without a sort step.
        // Arm 2 streams unscored items after arm 1 is exhausted.
        if pop_joined {
            let period = pop_period.unwrap();
            // CROSS JOIN forces popularity_agg as the outer loop (SQLite docs:
            // "CROSS JOIN prevents the optimizer from rearranging table order").
            // This guarantees SQLite walks idx_pop_agg_covering in avg-DESC order
            // and probes the filtered CTE by PK, producing scored items in score
            // order without a sort step.
            records_qb.push(format!(
                ") SELECT m.* FROM (\
                 SELECT f.* FROM popularity_agg pop CROSS JOIN filtered f \
                 WHERE f.id = pop.media_id AND pop.period = '{period}' AND pop.latest = 1 \
                 UNION ALL \
                 SELECT f.* FROM filtered f \
                 WHERE NOT EXISTS (\
                     SELECT 1 FROM popularity_agg p \
                     WHERE p.media_id = f.id AND p.period = '{period}' AND p.latest = 1\
                 )\
                ) m"
            ));
        }

        // Apply ORDER BY driven by the sort_by field, with per-kind fallbacks.
        let is_channel_query = filter
            .kind
            .as_ref()
            .map(|k| {
                !k.is_empty()
                    && k.iter()
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
                    // BLOB literal for correlated user_media_state lookups in
                    // ORDER BY (user-data sorts: PlayCount, IsPlayed, ...).
                    let user_hex = filter
                        .user_id
                        .as_ref()
                        .map(|u| format!("X'{}'", u.simple()));
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
                        api::ItemSortBy::CriticRating => {
                            format!("COALESCE(rating_critic, 0) {}", dir)
                        }
                        api::ItemSortBy::AiredEpisodeOrder => {
                            format!(
                                "COALESCE(parent_idx, 999999) {dir}, COALESCE(idx, 999999) {dir}"
                            )
                        }
                        api::ItemSortBy::OfficialRating => {
                            format!("COALESCE(certification_age, 999999) {}", dir)
                        }
                        api::ItemSortBy::Artist => {
                            // Artist name: grandparent row for tracks, parent row for
                            // albums, own title for artist rows.
                            format!(
                                "CASE WHEN kind = 'track' THEN \
                                   COALESCE((SELECT g.title FROM media g WHERE g.id = media.grandparent_id), '') \
                                 WHEN kind = 'album' THEN \
                                   COALESCE((SELECT p.title FROM media p WHERE p.id = media.parent_id), '') \
                                 ELSE COALESCE(title, '') END COLLATE NOCASE {}",
                                dir
                            )
                        }
                        api::ItemSortBy::AlbumArtist => {
                            // Album artist: the artist of the album (parent row for
                            // albums, grandparent for tracks).
                            format!(
                                "CASE WHEN kind = 'track' THEN \
                                   COALESCE((SELECT g.title FROM media g WHERE g.id = media.grandparent_id), '') \
                                 WHEN kind = 'album' THEN \
                                   COALESCE((SELECT p.title FROM media p WHERE p.id = media.parent_id), '') \
                                 ELSE COALESCE(title, '') END COLLATE NOCASE {}",
                                dir
                            )
                        }
                        api::ItemSortBy::Album => {
                            // Album title: parent row for tracks, own title for albums.
                            format!(
                                "CASE WHEN kind = 'track' THEN \
                                   COALESCE((SELECT p.title FROM media p WHERE p.id = media.parent_id), '') \
                                 ELSE COALESCE(title, '') END COLLATE NOCASE {}",
                                dir
                            )
                        }
                        api::ItemSortBy::SeriesSortName => {
                            // Series name: grandparent row for episodes, parent row
                            // for seasons, own title for everything else.
                            format!(
                                "CASE WHEN kind = 'episode' THEN \
                                   COALESCE((SELECT g.title FROM media g WHERE g.id = media.grandparent_id), '') \
                                 WHEN kind = 'season' THEN \
                                   COALESCE((SELECT p.title FROM media p WHERE p.id = media.parent_id), '') \
                                 ELSE COALESCE(title, '') END COLLATE NOCASE {}",
                                dir
                            )
                        }
                        api::ItemSortBy::DateLastContentAdded => {
                            // For series/seasons: the most recent season/episode
                            // creation date; for other items, their own date.
                            format!(
                                "COALESCE((SELECT MAX(c.created_at) FROM media c \
                                   WHERE (c.parent_id = media.id OR c.grandparent_id = media.id) \
                                   AND c.kind IN ('season','episode')), media.created_at) {}",
                                dir
                            )
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
                            if use_watched_cte {
                                // CTE path: watched_dates is already JOINed into the query,
                                // wd.effective_date holds the pre-computed effective play date
                                // (rolling episode plays up to grandparent series).
                                let null_date = "0001-01-01 00:00:00";
                                format!("COALESCE(wd.effective_date, '{null_date}') {dir}")
                            } else if filter.user_id.is_some() {
                                // dp alias from the UMS-driven records_qb above.
                                format!("dp.last_played_at {}", dir)
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::PlayCount => {
                            if let Some(uid) = &user_hex {
                                format!(
                                    "COALESCE((SELECT ums.play_count FROM user_media_state ums \
                                     WHERE ums.user_id = {uid} AND ums.media_id = media.id), 0) {}",
                                    dir
                                )
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::IsPlayed => {
                            if let Some(uid) = &user_hex {
                                // Played items first (ASC); NULL play state counts as unplayed.
                                format!(
                                    "CASE WHEN COALESCE((SELECT ums.play_count FROM user_media_state ums \
                                     WHERE ums.user_id = {uid} AND ums.media_id = media.id), 0) > 0 \
                                     THEN 0 ELSE 1 END {}",
                                    dir
                                )
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::IsUnplayed => {
                            if let Some(uid) = &user_hex {
                                // Unplayed items first (ASC).
                                format!(
                                    "CASE WHEN COALESCE((SELECT ums.play_count FROM user_media_state ums \
                                     WHERE ums.user_id = {uid} AND ums.media_id = media.id), 0) > 0 \
                                     THEN 1 ELSE 0 END {}",
                                    dir
                                )
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::IsFavoriteOrLiked => {
                            if let Some(uid) = &user_hex {
                                format!(
                                    "COALESCE((SELECT ums.favorite FROM user_media_state ums \
                                     WHERE ums.user_id = {uid} AND ums.media_id = media.id), 0) {}",
                                    dir
                                )
                            } else {
                                format!("title COLLATE NOCASE {}", dir)
                            }
                        }
                        api::ItemSortBy::Random => "RANDOM()".to_string(),
                        api::ItemSortBy::ChannelOrder => {
                            format!("(sort_order IS NULL), COALESCE(sort_order, channel_number, 999999) {dir}, title COLLATE NOCASE")
                        }
                        api::ItemSortBy::DisplayOrder => {
                            format!("(sort_order IS NULL), COALESCE(sort_order, 999999) {dir}, title COLLATE NOCASE")
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
            // When pop_joined, ordering is handled inside the UNION ALL arms —
            // arm 1 walks idx_pop_agg_covering in avg-DESC order, arm 2 follows.
            // Pushing ORDER BY here would force a global sort over the whole result.
            if !pop_joined {
                records_qb.push(" ORDER BY ");
                records_qb.push(order_clauses.join(", "));
            }
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
        } else {
            // Universal fallback: sort by index numbers so episodes/seasons/tracks
            // always come back in natural order when the client sends no SortBy.
            // Indexed content (episodes, seasons, tracks) has idx set; non-indexed
            // content (movies, series) gets COALESCE to 9999 and falls back to title.
            records_qb.push(
                " ORDER BY COALESCE(parent_idx, 9999) ASC, COALESCE(idx, 9999) ASC, title COLLATE NOCASE ASC",
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

        // exclude_childless needs child counts to decide what to drop; force them on.
        let effective_child_count =
            filter.include_child_count || filter.exclude_childless;

        // policy_filter scopes child-count queries to what the user can see.
        // Callers that have a user context (get_by_jellyfin_filter, items handlers)
        // pass it in; fall back to None when no user is involved.
        let child_policy_filter = filter
            .policy_filter
            .as_ref();

        if effective_child_count && !records.is_empty() {
            let folder_ids: Vec<Uuid> = records
                .iter()
                .filter(|m| {
                    matches!(
                        m.kind,
                        MediaKind::Series
                            | MediaKind::Season
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
                cc_qb.push(")");
                if let Some(pf) = child_policy_filter {
                    apply_filter_rules(
                        &mut cc_qb,
                        pf,
                        filter
                            .user_id
                            .as_ref(),
                        false,
                    );
                }
                cc_qb.push(" GROUP BY parent_id");
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
                if let Some(pf) = child_policy_filter {
                    pl_qb.push(
                        ") AND right_media_id IN (SELECT id FROM media WHERE 1=1",
                    );
                    apply_filter_rules(
                        &mut pl_qb,
                        pf,
                        filter
                            .user_id
                            .as_ref(),
                        false,
                    );
                    pl_qb.push(")");
                } else {
                    pl_qb.push(")");
                }
                pl_qb.push(" GROUP BY left_media_id");
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

            // Batch child counts for smart/catalog collections in a single UNION ALL query
            // instead of one COUNT per collection (N+1). Collect the needed data first so
            // the immutable borrow on records is released before we write back.
            let smart_coll_data: Vec<(
                Uuid,
                Option<Vec<MediaKind>>,
                Option<remux_sdks::remux::CollectionFilter>,
            )> = records
                .iter()
                .filter(|m| {
                    m.kind == MediaKind::Collection
                        && matches!(m.collection_kind, Some(CollectionKind::Smart))
                })
                .map(|m| {
                    let kinds = m
                        .collection_media_kind
                        .as_ref()
                        .map(|k| match k {
                            CollectionMediaKind::Movie => vec![MediaKind::Movie],
                            CollectionMediaKind::Series => vec![MediaKind::Series],
                            CollectionMediaKind::Mixed => {
                                vec![MediaKind::Movie, MediaKind::Series]
                            }
                            CollectionMediaKind::Music => {
                                vec![
                                    MediaKind::Track,
                                    MediaKind::Album,
                                    MediaKind::Artist,
                                ]
                            }
                            CollectionMediaKind::Playlist => vec![MediaKind::Playlist],
                            CollectionMediaKind::Collection => {
                                vec![MediaKind::Collection]
                            }
                        });
                    (
                        m.id,
                        kinds,
                        m.parse_smart_filter()
                            .cloned(),
                    )
                })
                .collect();

            if !smart_coll_data.is_empty() {
                let mut qb = sqlx::QueryBuilder::new("");
                for (n, (id, kinds, sf)) in smart_coll_data
                    .iter()
                    .enumerate()
                {
                    if n > 0 {
                        qb.push(" UNION ALL ");
                    }
                    qb.push("SELECT ");
                    qb.push_bind(*id);
                    qb.push(", COUNT(*) FROM media WHERE 1=1");
                    if let Some(ks) = kinds {
                        if !ks.is_empty() {
                            qb.push(" AND kind IN (");
                            let mut sep = qb.separated(", ");
                            for k in ks {
                                sep.push_bind(k.clone());
                            }
                            qb.push(")");
                        }
                    }
                    if let Some(sf) = sf {
                        apply_filter_rules(
                            &mut qb,
                            sf,
                            filter
                                .user_id
                                .as_ref(),
                            false,
                        );
                    }
                    if let Some(pf) = child_policy_filter {
                        apply_filter_rules(
                            &mut qb,
                            pf,
                            filter
                                .user_id
                                .as_ref(),
                            false,
                        );
                    }
                }
                match qb
                    .build()
                    .fetch_all(db)
                    .await
                {
                    Ok(rows) => {
                        let mut cc_map: HashMap<Uuid, i64> = HashMap::new();
                        for row in &rows {
                            cc_map.insert(row.get(0), row.get(1));
                        }
                        for media in &mut records {
                            if let Some(&cnt) = cc_map.get(&media.id) {
                                media.child_count = Some(cnt);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("failed to batch child counts for smart collections: {e}")
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

        // Drop empty containers when requested. child_count is already populated
        // for all container kinds (including smart/catalog) by the branches above.
        // Structural "collection of collections" containers always show.
        let sql_total = count?;

        if filter.exclude_childless {
            records.retain(|m| {
                if !matches!(
                    m.kind,
                    MediaKind::Collection | MediaKind::Folder | MediaKind::Playlist
                ) {
                    return true;
                }
                if m.is_group_container() {
                    return true;
                }
                m.child_count
                    .map_or(true, |c| c > 0)
            });
        }

        let total_count = if filter.exclude_childless {
            records.len()
        } else {
            sql_total
        };
        Ok(FilterResult {
            records,
            total_count,
        })
    }

    pub async fn get_refreshable(
        db: &SqlitePool,
        limit: u32,
        after_id: Option<Uuid>,
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

        // Cursor-based pagination: WHERE id > after_id avoids the OFFSET bug where
        // processed items shift out of the ORDER BY position causing skips/re-reads.
        let rows = if let Some(cursor) = after_id {
            sqlx::query_as::<_, Self>(&format!(
                "SELECT * FROM media {WHERE} AND id > ? ORDER BY id LIMIT ?"
            ))
            .bind(MediaKind::Movie)
            .bind(MediaKind::Series)
            .bind(cursor)
            .bind(limit)
            .fetch_all(db)
            .await?
        } else {
            sqlx::query_as::<_, Self>(&format!(
                "SELECT * FROM media {WHERE} ORDER BY id LIMIT ?"
            ))
            .bind(MediaKind::Movie)
            .bind(MediaKind::Series)
            .bind(limit)
            .fetch_all(db)
            .await?
        };

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

        // Revert of #178: singles/EPs show in the Albums section again (they were
        // hidden, leaving artist pages empty for single-only artists — #208). The
        // album_kinds column/filter machinery stays for compatibility but is not
        // applied here.
        let mut result = Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(kinds),
                album_kinds: None,
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
                // Always pass the policy filter so child-count queries inside
                // get_by_filter can scope counts to what the user sees. The main
                // WHERE clause in get_by_filter skips it for container-only queries
                // (CLAUDE.md: content rules must not filter containers themselves).
                policy_filter: user_policy_filter.cloned(),
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
                exclude_childless: targeting_containers
                    && !filter
                        .include_childless
                        .unwrap_or(false),
                ..Default::default()
            },
        )
        .await?;

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
        state.playback_position = 0;
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
                    .filter_map(|url| {
                        crate::stream::TrackerUrl::try_new(url.to_string()).ok()
                    })
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
            addon_id: None,
            catchup_source: None,
            catchup_days: None,
            usenet_guid: None,
            usenet_indexer: None,
            nzb_url: None,
            binge_group: None,
            stream_addon: None,
            torrent_info_hash: None,
            torrent_file_idx: None,
            service_id: None,
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

        // No IMDB ID means we can't resolve release dates against an external
        // provider (TMDB) either. If the addon reports no date at all for it,
        // treat it as available now rather than leaving digital_released_at
        // unset — an unset date means "unreleased" to push_release_date_filter,
        // which would hide the item forever for addons that never report dates.
        let has_no_imdb_id = meta
            .imdb_id
            .is_none()
            && ExternalIds::from_stremio_id(&meta.id)
                .imdb
                .is_none();

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
            })
            .or_else(|| {
                if has_no_imdb_id
                    && meta
                        .released
                        .is_none()
                {
                    Some(Utc::now().naive_utc())
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
                    sdks::stremio::Status::Other => MediaStatus::Continuing,
                })
                .or_else(|| {
                    // Fall back to release_info range when the addon omits status.
                    // Only meaningful for series; movies don't use this field.
                    if matches!(media_kind, MediaKind::Series) {
                        match meta
                            .release_info
                            .as_ref()
                        {
                            Some(sdks::stremio::ReleaseInfo::Ended { .. }) => {
                                Some(MediaStatus::Ended)
                            }
                            Some(sdks::stremio::ReleaseInfo::Ongoing { .. }) => {
                                Some(MediaStatus::Continuing)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                });

        // Derive series end_date only when we resolved Ended status.
        // Use the latest regular (non-specials) episode date, constrained by the
        // declared end year when present. Falls back to a synthetic year-end date
        // when no episode date is found but an end year was declared.
        let end_date: Option<NaiveDateTime> = if matches!(media_kind, MediaKind::Series)
            && matches!(status, Some(MediaStatus::Ended))
        {
            let declared_end_year = meta
                .release_info
                .as_ref()
                .and_then(|ri| ri.end_year());
            let from_episodes = meta
                .videos
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter(|ep| {
                    ep.season
                        .map_or(true, |s| s > 0)
                })
                .filter_map(|ep| ep.released)
                .filter(|dt| declared_end_year.map_or(true, |y| dt.year() <= y))
                .max()
                .map(|dt| dt.naive_utc());
            from_episodes.or_else(|| {
                declared_end_year.map(|y| {
                    chrono::NaiveDate::from_ymd_opt(y, 12, 31)
                        .unwrap_or_default()
                        .and_hms_opt(0, 0, 0)
                        .unwrap_or_default()
                })
            })
        } else {
            None
        };

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
                ids.custom_stremio_type = custom_stremio_type(&meta.media_type);
                ids
            },
            status,
            end_date,
            trailers: meta
                .trailers
                .map(|trailers| {
                    trailers
                        .into_iter()
                        .filter_map(|t| t.source)
                        .collect::<Vec<String>>()
                }),
            id: Uuid::new_v4(),
            ..Default::default()
        };

        let mut media = media;
        {
            let raw = super::MediaIdRaw {
                kind: media
                    .kind
                    .clone(),
                external_ids: media
                    .external_ids
                    .clone(),
                season: None,
                episode: None,
            };
            if raw
                .canonical()
                .is_some()
            {
                media.id = Uuid::from(&raw);
            }
        }
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
        media
            .external_ids
            .custom_stremio_id = Some(custom_id.clone());
        {
            let raw = super::MediaIdRaw {
                kind: media
                    .kind
                    .clone(),
                external_ids: media
                    .external_ids
                    .clone(),
                season: None,
                episode: None,
            };
            media.id = raw
                .canonical()
                .map(|_| Uuid::from(&raw))
                .unwrap_or_else(Uuid::new_v4);
        }
        let series_key = media.series_canonical_key();
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
                    let season_id = Media::season_id(&series_key, season_idx);
                    let mut season = Media {
                        id: season_id,
                        title: format!("Season {}", season_idx),
                        kind: MediaKind::Season,
                        idx: Some(season_idx),
                        parent_id: Some(media.id),
                        grandparent_id: Some(media.id),
                        external_ids: ExternalIds {
                            custom_stremio_type: media
                                .external_ids
                                .custom_stremio_type
                                .clone(),
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
                        episode.idx = ep.episode;
                        episode.id = Media::episode_id(&series_key, season_idx, ep_idx);
                        episode.external_ids = ExternalIds {
                            custom_stremio_type: media
                                .external_ids
                                .custom_stremio_type
                                .clone(),
                            custom_stremio_id: Some(
                                ep.id
                                    .clone(),
                            ),
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

    {
        let raw = super::MediaIdRaw {
            kind: media
                .kind
                .clone(),
            external_ids: media
                .external_ids
                .clone(),
            season: None,
            episode: None,
        };
        media.id = raw
            .canonical()
            .map(|_| Uuid::from(&raw))
            .unwrap_or_else(Uuid::new_v4);
    }
    let series_key = media.series_canonical_key();

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
                let season_id = Media::season_id(&series_key, season_idx);
                let mut season = Media {
                    id: season_id,
                    title: format!("Season {}", season_idx),
                    kind: MediaKind::Season,
                    idx: Some(season_idx),
                    grandparent_id: Some(media.id),
                    external_ids: ExternalIds {
                        custom_stremio_type: media
                            .external_ids
                            .custom_stremio_type
                            .clone(),
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
                    episode.idx = ep.episode;
                    episode.id = Media::episode_id(&series_key, season_idx, ep_idx);
                    episode.external_ids = ExternalIds {
                        custom_stremio_type: media
                            .external_ids
                            .custom_stremio_type
                            .clone(),
                        custom_stremio_id: Some(
                            ep.id
                                .clone(),
                        ),
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
    let series_key = Media::series_canonical_key_ext(series_external_ids)
        .unwrap_or_else(|| series_id.to_string());
    let has_canonical_key =
        Media::series_canonical_key_ext(series_external_ids).is_some();

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
        if !has_canonical_key {
            continue;
        }
        // UUID anchored to the stable series UUID + season index — no series_* fields needed.
        let season_id = Media::season_id(&series_key, season_idx);
        let external_ids = ExternalIds {
            custom_stremio_type: series_external_ids
                .custom_stremio_type
                .clone(),
            ..Default::default()
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
        out.push(stremio_meta_episode(
            ep,
            series_id,
            season_id,
            season_idx,
            series_external_ids,
        )?);
    }
    Ok(out)
}

/// Build a single episode `Media` from one Stremio `videos[]` entry.
///
/// Split out of `stremio_meta_season_episodes` so per-episode meta refresh can
/// convert just the video it needs instead of materialising the whole season and
/// discarding all but one row (quadratic on series with thousands of episodes).
pub fn stremio_meta_episode(
    ep: &crate::sdks::stremio::Episode,
    series_id: Uuid,
    season_id: Uuid,
    season_idx: i64,
    series_external_ids: &ExternalIds,
) -> Result<Media> {
    let ep_idx = ep
        .episode
        .unwrap_or(0);
    let series_key = Media::series_canonical_key_ext(series_external_ids)
        .unwrap_or_else(|| series_id.to_string());
    let mut episode: Media = ep
        .clone()
        .try_into()?;

    if series_external_ids
        .imdb
        .is_some()
        || series_external_ids
            .custom_stremio_id
            .is_some()
    {
        episode.external_ids = ExternalIds {
            custom_stremio_type: series_external_ids
                .custom_stremio_type
                .clone(),
            custom_stremio_id: Some(
                ep.id
                    .clone(),
            ),
            ..Default::default()
        };
        // UUID anchored to stable canonical series key + season/episode indices
        // (flat) so it survives a purge + repopulate even if the series UUID
        // itself was derived differently.
        episode.id = Media::episode_id(&series_key, season_idx, ep_idx);
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

    Ok(episode)
}

/// Push the release-date WHERE condition onto a query builder, binding `threshold`.
///
/// `alias` is the table alias for the media row (e.g. `"media"` for an unaliased
/// table, `"e"` when episodes are selected as `media e`).
///
/// Hides items whose resolved release date is after `threshold`. Resolution priority:
/// 1. `digital_released_at` — explicit digital/streaming date; used as-is.
/// 2. Movies with `released_at` within the past year and no digital date → hidden.
///    A recent theatrical release with no confirmed digital date is still considered
///    unreleased digitally. TV air dates remain valid for episodes.
/// 3. ELSE — depends on `use_parent_fallback`:
///    - `true`  (episodes): fall back to the parent row's dates via a correlated
///      subquery so undated episodes of old series inherit the series premiere.
///    - `false` (movies, series, seasons): use `released_at` directly. Movies and
///      series have NULL parent_id so the subquery would always return NULL anyway;
///      seasons must not inherit the series premiere (TVDB lists future seasons early).
///
/// Items with no resolvable date are always excluded (the OR condition is false for them).
///
/// The filter is expressed as OR branches rather than a CASE expression so that SQLite
/// can use `idx_media_digital_released_at` for branch 1 and `idx_media_released_at`
/// for branch 2, avoiding a full table scan.
pub fn push_release_date_filter(
    qb: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    alias: &str,
    threshold: NaiveDateTime,
    use_parent_fallback: bool,
) {
    let a = format!("{alias}.");
    if use_parent_fallback {
        // Episodes: branch 2 falls back to the parent row's date when released_at
        // is NULL. The correlated subquery is cheap here because the episode set
        // is already narrow (filtered by parent_id / season).
        qb.push(format!(
            " AND (({a}digital_released_at IS NOT NULL AND {a}digital_released_at <= "
        ))
        .push_bind(threshold)
        .push(format!(
            ") OR ({a}digital_released_at IS NULL \
              AND NOT ({a}kind = 'movie' AND {a}released_at IS NOT NULL AND {a}released_at > date('now', '-1 year')) \
              AND COALESCE({a}released_at, \
                (SELECT COALESCE(p.digital_released_at, p.released_at) FROM media p WHERE p.id = {a}parent_id) \
              ) IS NOT NULL \
              AND COALESCE({a}released_at, \
                (SELECT COALESCE(p.digital_released_at, p.released_at) FROM media p WHERE p.id = {a}parent_id) \
              ) <= "
        ))
        .push_bind(threshold)
        .push("))");
    } else {
        // Movies / series / seasons: no parent fallback. Each branch is indexable.
        // Branch 1 → idx_media_digital_released_at
        // Branch 2 → idx_media_released_at
        qb.push(format!(
            " AND (({a}digital_released_at IS NOT NULL AND {a}digital_released_at <= "
        ))
        .push_bind(threshold)
        .push(format!(
            ") OR ({a}digital_released_at IS NULL \
              AND {a}released_at IS NOT NULL \
              AND {a}released_at <= "
        ))
        .push_bind(threshold)
        .push(format!(
            " AND NOT ({a}kind = 'movie' AND {a}released_at > date('now', '-1 year'))))"
        ));
    }
}

/// Append WHERE clauses for a set of `FilterRule`s onto a query builder.
///
/// Called once for both the count and records builders inside `get_by_filter`.
///
/// # SQL strategy per field
/// - `year` / `rating_*` / `certification` — direct column comparison
/// - `tag` — `media.id IN (SELECT media_id FROM media_tags WHERE ...)`
/// - `genre` / `studio` / `country` / `person` — `media.id IN (SELECT left_media_id FROM media_relations JOIN media WHERE ...)`
/// - `catalog` / `collection_member` — `media.id IN (SELECT right_media_id FROM media_relations WHERE ...)`
/// - `has_trailer` — json_array_length check
pub fn apply_filter_rules(
    qb: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    filter: &remux_sdks::remux::CollectionFilter,
    user_id: Option<&Uuid>,
    skip_watched_true: bool,
) {
    use remux_sdks::remux::FilterMatchMode;

    let group_sep = match filter.match_mode {
        FilterMatchMode::All => " AND ",
        FilterMatchMode::Any => " OR ",
    };

    // Pre-compute SQL for every rule so groups with no valid rules are skipped.
    // filter_rule_to_sql returns None for rules with empty value lists, which
    // would otherwise produce `()` — an invalid SQLite IN clause.
    let valid_groups: Vec<(_, Vec<(String, bool)>)> = filter
        .groups
        .iter()
        .filter_map(|g| {
            let rules: Vec<_> = g
                .rules
                .iter()
                .filter_map(|r| filter_rule_to_sql(r, user_id, skip_watched_true))
                .collect();
            if rules.is_empty() {
                None
            } else {
                Some((g, rules))
            }
        })
        .collect();

    if valid_groups.is_empty() {
        return;
    }

    qb.push(" AND (");
    let mut first_group = true;
    for (group, rules) in valid_groups {
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
        for (sql, negated) in rules {
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
        qb.push(")");
    }
    qb.push(")");
}

/// Build a self-contained SQL fragment string from a `CollectionFilter`.
///
/// Unlike `apply_filter_rules`, this returns a `String` rather than pushing to a
/// `QueryBuilder`. This allows the fragment to be embedded as a subquery (e.g.
/// `grandparent_id IN (SELECT id FROM media WHERE kind = 'series' AND <fragment>)`).
/// All values are escaped inline via `esc()` — no bound parameters.
///
/// Returns `None` when there are no valid rule groups, meaning the filter places
/// no restriction (all items qualify).
fn build_filter_rules_fragment(
    filter: &remux_sdks::remux::CollectionFilter,
    user_id: Option<&Uuid>,
    skip_watched_true: bool,
) -> Option<String> {
    use remux_sdks::remux::FilterMatchMode;

    let group_sep = match filter.match_mode {
        FilterMatchMode::All => " AND ",
        FilterMatchMode::Any => " OR ",
    };

    let valid_groups: Vec<_> = filter
        .groups
        .iter()
        .filter_map(|g| {
            let rules: Vec<_> = g
                .rules
                .iter()
                .filter_map(|r| filter_rule_to_sql(r, user_id, skip_watched_true))
                .collect();
            if rules.is_empty() {
                None
            } else {
                Some((g, rules))
            }
        })
        .collect();

    if valid_groups.is_empty() {
        return None;
    }

    let group_strs: Vec<String> = valid_groups
        .iter()
        .map(|(g, rules)| {
            let rule_sep = match g.match_mode {
                FilterMatchMode::All => " AND ",
                FilterMatchMode::Any => " OR ",
            };
            let rule_strs: Vec<String> = rules
                .iter()
                .map(|(sql, negated)| {
                    if *negated {
                        format!("NOT ({sql})")
                    } else {
                        sql.clone()
                    }
                })
                .collect();
            format!("({})", rule_strs.join(rule_sep))
        })
        .collect();

    Some(format!("({})", group_strs.join(group_sep)))
}

/// Translate one `FilterRule` into a raw SQL fragment.
///
/// Values are embedded directly — no string parsing needed since the rule carries typed values.
/// Returns `(sql, negated)` — caller wraps in `NOT(...)` when negated is true.
/// Returns `None` if the rule should be skipped (e.g. empty values list).
fn filter_rule_to_sql(
    rule: &remux_sdks::remux::FilterRule,
    user_id: Option<&Uuid>,
    skip_watched_true: bool,
) -> Option<(String, bool)> {
    use remux_sdks::remux::{FilterRule as R, NumericOp, SetOp};

    if skip_watched_true {
        if matches!(rule, R::Played { value: true }) {
            return None;
        }
    }

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
                NumericOp::Gt => format!(
                    "COALESCE(certification_age, (SELECT p.certification_age FROM media p WHERE p.id = COALESCE(media.grandparent_id, media.parent_id))) > {value}"
                ),
                NumericOp::Lt => format!(
                    "COALESCE(certification_age, (SELECT p.certification_age FROM media p WHERE p.id = COALESCE(media.grandparent_id, media.parent_id))) <= {value}"
                ),
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
                        "media.id IN (SELECT mt.media_id FROM media_tags mt WHERE lower(mt.tag) = lower('{v}'))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "media.id IN (SELECT mt.media_id FROM media_tags mt WHERE lower(mt.tag) IN ({list}))"
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
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         JOIN media p ON p.id = mr.right_media_id \
                         WHERE p.kind = 'person' AND lower(p.title) = lower('{v}'))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "media.id IN (SELECT mr.left_media_id FROM media_relations mr \
                         JOIN media p ON p.id = mr.right_media_id \
                         WHERE p.kind = 'person' AND lower(p.title) IN ({list}))"
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
                "media.id IN (SELECT mr.right_media_id FROM media_relations mr \
                 WHERE mr.role = 'catalog' AND mr.left_media_id IN ({in_clause}))"
            );
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            Some((sql, negated))
        }
        R::Catalog { .. } => None,
        R::GroupContainer { value } => {
            let sql = "media.parent_id IS NOT NULL".to_string();
            Some((sql, !value))
        }
        R::CollectionMember { op, collection_ids } if !collection_ids.is_empty() => {
            let in_clause = collection_ids
                .iter()
                .map(|id| format!("X'{}'", id.simple()))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "media.id IN (SELECT mr.right_media_id FROM media_relations mr \
                 WHERE mr.role = 'collection' AND mr.left_media_id IN ({in_clause}))"
            );
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            Some((sql, negated))
        }
        R::CollectionMember { .. } => None,
        R::CollectionId { op, ids } if !ids.is_empty() => {
            let in_clause = ids
                .iter()
                .map(|id| format!("X'{}'", id.simple()))
                .collect::<Vec<_>>()
                .join(", ");
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            Some((format!("media.id IN ({in_clause})"), negated))
        }
        R::CollectionId { .. } => None,
        R::Favorite { value } => {
            let user_clause = user_id
                .map(|id| format!(" AND ums.user_id = X'{}'", id.simple()))
                .unwrap_or_default();
            let sql = if *value {
                format!(
                    "EXISTS (SELECT 1 FROM user_media_state ums \
                     WHERE ums.media_id = media.id{user_clause} AND ums.favorite = 1)"
                )
            } else {
                format!(
                    "NOT EXISTS (SELECT 1 FROM user_media_state ums \
                     WHERE ums.media_id = media.id{user_clause} AND ums.favorite = 1)"
                )
            };
            Some((sql, false))
        }
        R::Played { value } => {
            let user_clause = user_id
                .map(|id| format!(" AND ums.user_id = X'{}'", id.simple()))
                .unwrap_or_default();
            let sql = if *value {
                format!(
                    "media.id IN (\
                      SELECT ums.media_id FROM user_media_state ums \
                      WHERE ums.play_count > 0{user_clause} \
                      UNION \
                      SELECT ep.grandparent_id FROM user_media_state ums \
                      JOIN media ep ON ep.id = ums.media_id \
                      WHERE ep.kind = 'episode' AND ep.grandparent_id IS NOT NULL \
                        AND ums.play_count > 0{user_clause}\
                    )"
                )
            } else {
                format!(
                    "NOT EXISTS (SELECT 1 FROM user_media_state ums \
                     WHERE ums.media_id = media.id{user_clause} AND ums.play_count > 0)"
                )
            };
            Some((sql, false))
        }
        R::MediaKind { op, values } if !values.is_empty() => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let mut db_kinds: Vec<&'static str> = Vec::new();
            for v in values {
                match v.as_str() {
                    "movie" => db_kinds.push("movie"),
                    "series" => {
                        db_kinds.extend_from_slice(&["series", "episode", "season"])
                    }
                    "music" => db_kinds.extend_from_slice(&[
                        "track",
                        "album",
                        "artist",
                        "music_genre",
                    ]),
                    "live_tv" => {
                        db_kinds.extend_from_slice(&["tv_channel", "tv_program"])
                    }
                    _ => {}
                }
            }
            if db_kinds.is_empty() {
                return None;
            }
            let list = db_kinds
                .iter()
                .map(|k| format!("'{k}'"))
                .collect::<Vec<_>>()
                .join(", ");
            Some((format!("media.kind IN ({list})"), negated))
        }
        R::MediaKind { .. } => None,
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

    /// `stremio_meta_episode` is the per-episode fast path used by meta refresh;
    /// it must produce exactly what the whole-season builder produces for the
    /// same video, otherwise refreshing an episode would rewrite it differently
    /// than the tree import created it.
    #[test]
    fn stremio_meta_episode_matches_season_builder() {
        let series_id = Uuid::from_u128(1);
        let season_id = Uuid::from_u128(2);
        let ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new("tt1234567".to_string()).unwrap()),
            ..Default::default()
        };
        let videos_json: Vec<_> = (1..=5)
            .map(|e| (1, e))
            .chain((1..=3).map(|e| (2, e)))
            .map(|(s, e)| {
                serde_json::json!({
                    "id": format!("tt1234567:{s}:{e}"),
                    "title": format!("Episode {e}"),
                    "season": s,
                    "episode": e,
                    "thumbnail": "https://example.invalid/thumb.jpg",
                    "overview": "overview",
                })
            })
            .collect();
        let meta: sdks::stremio::Meta = serde_json::from_value(serde_json::json!({
            "id": "tt1234567",
            "type": "series",
            "name": "Test Series",
            "imdb_id": "tt1234567",
            "videos": videos_json,
        }))
        .expect("fixture meta deserializes");
        let videos = meta
            .videos
            .clone()
            .expect("videos present");

        let whole_season =
            stremio_meta_season_episodes(&meta, series_id, season_id, 1, &ext).unwrap();
        assert_eq!(whole_season.len(), 5, "season 1 should yield 5 episodes");

        for expected in &whole_season {
            let video = videos
                .iter()
                .find(|v| v.episode == expected.idx && v.season == Some(1))
                .expect("fixture video exists");
            let single =
                stremio_meta_episode(video, series_id, season_id, 1, &ext).unwrap();
            assert_eq!(single.id, expected.id);
            assert_eq!(single.idx, expected.idx);
            assert_eq!(single.parent_idx, expected.parent_idx);
            assert_eq!(single.parent_id, expected.parent_id);
            assert_eq!(single.grandparent_id, expected.grandparent_id);
            assert_eq!(single.title, expected.title);
            assert_eq!(
                single
                    .external_ids
                    .custom_stremio_id,
                expected
                    .external_ids
                    .custom_stremio_id
            );
        }
    }

    /// Regression test for #235: episode/season UUIDs must be anchored to the
    /// series' canonical external-ID string (imdb ▸ custom ▸ tmdb ▸ …), not to
    /// the series' own UUID. After a purge + repopulate the series can be
    /// re-derived with a different UUID, which previously cascaded into every
    /// season/episode id and orphaned their user_media_state rows.
    #[test]
    fn episode_ids_survive_series_uuid_change() {
        let ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new("tt1234567".to_string()).unwrap()),
            ..Default::default()
        };
        let series_key = Media::series_canonical_key_ext(&ext).unwrap();
        let season_id = crate::common::stable_media_uuid(
            &MediaKind::Season,
            &format!("{series_key}:2"),
        );
        let video: sdks::stremio::Episode = serde_json::from_value(serde_json::json!({
            "id": "tt1234567:2:3",
            "title": "Episode 3",
            "season": 2,
            "episode": 3,
        }))
        .expect("fixture video");
        let expected = crate::common::stable_media_uuid(
            &MediaKind::Episode,
            &format!("tt1234567:2:3"),
        );
        // Same series content imported under two different series UUIDs
        // (e.g. pre/post purge with different enrichment outcomes).
        for series_id in [Uuid::from_u128(11), Uuid::from_u128(22)] {
            let ep =
                stremio_meta_episode(&video, series_id, season_id, 2, &ext).unwrap();
            assert_eq!(
                ep.id, expected,
                "episode id must not depend on the series UUID"
            );
        }
    }

    /// The recall side of #235: episode state rows must be findable under every
    /// UUID the importer could have written — the nested parent-anchored scheme
    /// (old), flat external-ID keys (new), and the episode's own Stremio ID.
    #[test]
    fn episode_uuid_candidates_cover_nested_flat_and_stremio_id() {
        let series_id = Uuid::from_u128(77);
        let mut series = Media {
            id: series_id,
            kind: MediaKind::Series,
            external_ids: ExternalIds {
                imdb: Some(NonEmptyString::try_new("tt1234567".to_string()).unwrap()),
                custom_stremio_id: Some("fk:27".to_string()),
                tmdb: Some(12345),
                ..Default::default()
            },
            ..Default::default()
        };
        series.id = Uuid::from(&MediaIdRaw {
            kind: series
                .kind
                .clone(),
            external_ids: series
                .external_ids
                .clone(),
            season: None,
            episode: None,
        });

        let season_id = crate::common::stable_media_uuid(
            &MediaKind::Season,
            &format!("{}:1", series_id),
        );
        let episode = Media {
            id: crate::common::stable_media_uuid(&MediaKind::Episode, &"deadbeef"),
            kind: MediaKind::Episode,
            idx: Some(3),
            parent_idx: Some(1),
            parent_id: Some(season_id),
            grandparent_id: Some(series_id),
            external_ids: ExternalIds {
                custom_stremio_id: Some("fk-ep-3".to_string()),
                ..Default::default()
            },
            grandparent: Some(Box::new(series.clone())),
            ..Default::default()
        };

        let candidates = Media::ext_id_uuid_candidates(&episode);

        // (1) nested / old Stremio scheme
        let nested = crate::common::stable_media_uuid(
            &MediaKind::Episode,
            &format!("{season_id}:3"),
        );
        assert!(candidates.contains(&nested), "missing nested candidate");

        // (2) flat keys from every grandparent external ID
        for key in ["tt1234567:1:3", "fk:27:1:3", "tmdb:12345:1:3"] {
            let flat = crate::common::stable_media_uuid(&MediaKind::Episode, key);
            assert!(candidates.contains(&flat), "missing flat candidate {key}");
        }

        // (3) the episode's own Stremio ID
        let own = crate::common::stable_media_uuid(&MediaKind::Episode, "fk-ep-3");
        assert!(
            candidates.contains(&own),
            "missing own stremio-id candidate"
        );

        // The current id itself must never be returned as a candidate.
        assert!(!candidates.contains(&episode.id));
    }

    #[test]
    fn season_uuid_candidates_cover_nested_and_flat() {
        let series_id = Uuid::from_u128(88);
        let series = Media {
            id: series_id,
            kind: MediaKind::Series,
            external_ids: ExternalIds {
                imdb: Some(NonEmptyString::try_new("tt1234567".to_string()).unwrap()),
                custom_stremio_id: Some("fk:27".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let season = Media {
            id: crate::common::stable_media_uuid(&MediaKind::Season, &"deadbeef"),
            kind: MediaKind::Season,
            idx: Some(1),
            parent_id: Some(series_id),
            grandparent_id: Some(series_id),
            grandparent: Some(Box::new(series)),
            ..Default::default()
        };
        let candidates = Media::ext_id_uuid_candidates(&season);

        let nested = crate::common::stable_media_uuid(
            &MediaKind::Season,
            &format!("{series_id}:1"),
        );
        assert!(candidates.contains(&nested), "missing nested candidate");
        for key in ["tt1234567:1", "fk:27:1"] {
            let flat = crate::common::stable_media_uuid(&MediaKind::Season, key);
            assert!(candidates.contains(&flat), "missing flat candidate {key}");
        }
        assert!(!candidates.contains(&season.id));
    }

    #[test]
    fn custom_stremio_type_extracts_non_standard_type() {
        assert_eq!(
            custom_stremio_type(&sdks::stremio::MediaType::Other("anime".to_string())),
            Some("anime".to_string())
        );
        assert_eq!(custom_stremio_type(&sdks::stremio::MediaType::Series), None);
        assert_eq!(
            custom_stremio_type(&sdks::stremio::MediaType::Other(
                "episode".to_string()
            )),
            None
        );
    }

    fn track(
        artist_name: Option<&str>,
        album_title: Option<&str>,
        description: Option<&str>,
    ) -> Media {
        Media {
            title: "Hello".to_string(),
            description: description.map(String::from),
            external_ids: ExternalIds {
                artist_name: artist_name.map(String::from),
                album_title: album_title.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn artist_name_prefers_grandparent_row() {
        let mut media = track(Some("Stale"), None, None);
        media.grandparent = Some(Media::stub(Uuid::new_v4(), "Adele"));
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_name_falls_back_to_flat_for_playlist_imports() {
        let media = track(Some("Adele"), None, None);
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_name_falls_back_to_description_prefix() {
        let media = track(None, None, Some("by Adele"));
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_name_prefers_flat_over_description() {
        let media = track(Some("Adele"), None, Some("by Someone Else"));
        assert_eq!(media.artist_name(), Some("Adele"));
    }

    #[test]
    fn artist_name_ignores_empty_names() {
        let media = track(Some(""), None, Some("by "));
        assert_eq!(media.artist_name(), None);
    }

    #[test]
    fn artist_name_from_external_parent_title() {
        let media = track(Some("Stale"), None, None);
        assert_eq!(media.artist_name_from(Some("Adele")), Some("Adele"));
        assert_eq!(media.artist_name_from(None), Some("Stale"));
    }

    #[test]
    fn album_name_prefers_parent_row() {
        let mut media = track(None, Some("Stale"), None);
        media.parent = Some(Media::stub(Uuid::new_v4(), "21"));
        assert_eq!(media.album_name(), Some("21"));
    }

    #[test]
    fn album_name_falls_back_to_flat_for_playlist_imports() {
        let media = track(None, Some("21"), None);
        assert_eq!(media.album_name(), Some("21"));
    }

    #[test]
    fn album_name_ignores_empty_titles() {
        let media = track(None, Some(""), None);
        assert_eq!(media.album_name(), None);
    }

    #[test]
    fn full_title_uses_flat_artist_for_playlist_tracks() {
        let mut media = track(Some("Adele"), None, None);
        media.kind = MediaKind::Track;
        assert_eq!(media.full_title(), "Adele - Hello");
    }

    #[test]
    fn track_search_query_uses_artist_and_title() {
        let media = track(Some("Adele"), None, None);
        assert_eq!(media.track_search_query(), "Adele Hello");
    }

    #[test]
    fn track_search_query_prefers_grandparent_row() {
        let mut media = track(Some("Stale"), None, None);
        media.grandparent = Some(Media::stub(Uuid::new_v4(), "Adele"));
        assert_eq!(media.track_search_query(), "Adele Hello");
    }

    #[test]
    fn track_search_query_title_only_without_artist() {
        let media = track(None, None, None);
        assert_eq!(media.track_search_query(), "Hello");
    }

    #[test]
    fn track_search_query_from_external_artist_title() {
        let media = track(Some("Stale"), None, None);
        assert_eq!(media.track_search_query_from(Some("Adele")), "Adele Hello");
        assert_eq!(media.track_search_query_from(None), "Stale Hello");
    }

    #[test]
    fn deezer_search_query_pins_artist_and_kind() {
        let media = track(Some("Adele"), None, None);
        assert_eq!(
            media.deezer_search_query("track"),
            "artist:\"Adele\" track:\"Hello\""
        );
    }

    #[test]
    fn deezer_search_query_strips_quotes() {
        let media = track(Some("The \"Artist\""), None, None);
        assert_eq!(
            media.deezer_search_query("album"),
            "artist:\"The Artist\" album:\"Hello\""
        );
    }

    #[test]
    fn deezer_search_query_title_only_without_artist() {
        let media = track(None, None, None);
        assert_eq!(media.deezer_search_query("track"), "Hello");
    }

    #[test]
    fn stremio_media_type_prefers_custom_type_over_kind() {
        let anime_ids = ExternalIds {
            custom_stremio_type: Some("anime".to_string()),
            ..Default::default()
        };
        assert_eq!(
            anime_ids.stremio_media_type(&MediaKind::Series),
            sdks::stremio::MediaType::Other("anime".to_string())
        );

        let standard_ids = ExternalIds::default();
        assert_eq!(
            standard_ids.stremio_media_type(&MediaKind::Series),
            sdks::stremio::MediaType::Series
        );
    }

    #[test]
    fn stremio_meta_to_medias_propagates_custom_type_to_episodes() {
        let json = r#"{
            "id": "fk:27",
            "type": "anime",
            "name": "Bleach Yabai",
            "videos": [
                {"id": "fk:27:1:1", "season": 1, "episode": 1, "title": "Ep 1"},
                {"id": "fk:27:1:2", "season": 1, "episode": 2, "title": "Ep 2"}
            ]
        }"#;
        let meta: sdks::stremio::Meta = serde_json::from_str(json).unwrap();
        let medias = stremio_meta_to_medias(meta).unwrap();

        let series = medias
            .iter()
            .find(|m| m.kind == MediaKind::Series)
            .expect("series media");
        assert_eq!(
            series
                .external_ids
                .custom_stremio_type,
            Some("anime".to_string())
        );

        let season = medias
            .iter()
            .find(|m| m.kind == MediaKind::Season)
            .expect("season media");
        assert_eq!(
            season
                .external_ids
                .custom_stremio_type,
            Some("anime".to_string())
        );

        let episode = medias
            .iter()
            .find(|m| m.kind == MediaKind::Episode)
            .expect("episode media");
        assert_eq!(
            episode
                .external_ids
                .custom_stremio_type,
            Some("anime".to_string())
        );
    }

    #[test]
    fn stremio_meta_to_medias_captures_episode_video_id() {
        let json = r#"{
            "id": "fk:27",
            "type": "anime",
            "name": "Bleach Yabai",
            "videos": [
                {"id": "fk:27:1:1", "season": 1, "episode": 1, "title": "Ep 1"}
            ]
        }"#;
        let meta: sdks::stremio::Meta = serde_json::from_str(json).unwrap();
        let medias = stremio_meta_to_medias(meta).unwrap();

        let episode = medias
            .iter()
            .find(|m| m.kind == MediaKind::Episode)
            .expect("episode media");
        assert_eq!(
            episode
                .external_ids
                .custom_stremio_id,
            Some("fk:27:1:1".to_string())
        );
    }

    #[test]
    fn custom_id_meta_with_no_dates_stamps_digital_released_at_now() {
        let json = r#"{
            "id": "fk:27",
            "type": "anime",
            "name": "Bleach Yabai"
        }"#;
        let meta: sdks::stremio::Meta = serde_json::from_str(json).unwrap();
        let before = chrono::Utc::now().naive_utc();
        let media: Media = meta
            .try_into()
            .unwrap();
        let after = chrono::Utc::now().naive_utc();

        let stamped = media
            .digital_released_at
            .expect(
                "no-IMDB item with no dates must get a stamped digital_released_at",
            );
        assert!(
            stamped >= before && stamped <= after,
            "expected digital_released_at to be stamped to now; got {:?} (window {:?}..{:?})",
            stamped,
            before,
            after
        );
    }

    #[test]
    fn imdb_meta_with_no_dates_leaves_digital_released_at_unset() {
        let json = r#"{
            "id": "tt9990010",
            "type": "movie",
            "name": "Some Movie"
        }"#;
        let meta: sdks::stremio::Meta = serde_json::from_str(json).unwrap();
        let media: Media = meta
            .try_into()
            .unwrap();

        assert_eq!(
            media.digital_released_at, None,
            "items with a resolvable IMDB ID must not get a stamped date — \
             their date can still be resolved externally (e.g. via TMDB)"
        );
    }

    #[test]
    fn candidate_ids_movie_all_id_types() {
        let ext = ExternalIds {
            imdb: NonEmptyString::try_new("tt1234567".to_string()).ok(),
            custom_stremio_id: Some("custom:abc".into()),
            tmdb: Some(999),
            tvdb: Some(777),
            kitsu: Some(555),
            ..Default::default()
        };
        let ids = ext.candidate_ids(&MediaKind::Movie, None, None, None);
        assert_eq!(
            ids,
            vec![
                "tt1234567",
                "custom:abc",
                "tmdb:999",
                "tvdb:777",
                "kitsu:555"
            ]
        );
    }

    /// Regression: a Kitsu-imported series derives its UUID from
    /// `custom_stremio_id`, often its only identity. TMDB enrichment rebuilt
    /// `ExternalIds` from scratch, so `validate` then rejected the row and the
    /// series vanished from its library.
    #[test]
    fn kitsu_series_still_validates_after_tmdb_enrichment() {
        let mut media = Media {
            kind: MediaKind::Series,
            title: "Daemons of the Shadow Realm".to_string(),
            external_ids: ExternalIds::from_stremio_id("kitsu:50023"),
            ..Default::default()
        };
        media.id = Uuid::from(&media.media_id_raw());
        assert_eq!(
            media
                .id
                .to_string(),
            "cffb332d-0f99-5725-968d-26d2d5cc0e8c",
            "a kitsu-only series is keyed on its Stremio ID"
        );
        media
            .validate()
            .expect("freshly imported kitsu series must be valid");

        // Everything TMDB contributes; it knows nothing about kitsu.
        let tmdb_ids = ExternalIds {
            tmdb: Some(260463),
            imdb: NonEmptyString::try_new("tt37532356".to_string()).ok(),
            tvdb: Some(452711),
            ..Default::default()
        };
        media
            .external_ids
            .merge(&tmdb_ids, false);

        assert_eq!(
            media
                .external_ids
                .kitsu,
            Some(50023),
            "enrichment must not drop the ID the UUID was derived from"
        );
        media
            .validate()
            .expect("enrichment must not invalidate the row it just enriched");
    }

    #[test]
    fn candidate_ids_movie_imdb_only() {
        let ext = ExternalIds {
            imdb: NonEmptyString::try_new("tt9999999".to_string()).ok(),
            ..Default::default()
        };
        let ids = ext.candidate_ids(&MediaKind::Movie, None, None, None);
        assert_eq!(ids, vec!["tt9999999"]);
    }

    #[test]
    fn candidate_ids_season_with_grandparent() {
        let gp = ExternalIds {
            imdb: NonEmptyString::try_new("tt1844624".to_string()).ok(),
            tmdb: Some(123),
            ..Default::default()
        };
        let ext = ExternalIds::default();
        let ids = ext.candidate_ids(&MediaKind::Season, Some(2), None, Some(&gp));
        assert_eq!(ids, vec!["tt1844624:2", "tmdb:123:2"]);
    }

    #[test]
    fn candidate_ids_season_no_grandparent_returns_empty() {
        let ext = ExternalIds::default();
        let ids = ext.candidate_ids(&MediaKind::Season, Some(1), None, None);
        assert!(ids.is_empty());
    }

    #[test]
    fn candidate_ids_season_no_index_returns_empty() {
        let gp = ExternalIds {
            imdb: NonEmptyString::try_new("tt1844624".to_string()).ok(),
            ..Default::default()
        };
        let ext = ExternalIds::default();
        let ids = ext.candidate_ids(&MediaKind::Season, None, None, Some(&gp));
        assert!(ids.is_empty());
    }

    #[test]
    fn candidate_ids_episode_custom_stremio_id_comes_first() {
        let gp = ExternalIds {
            imdb: NonEmptyString::try_new("tt1844624".to_string()).ok(),
            ..Default::default()
        };
        let ext = ExternalIds {
            custom_stremio_id: Some("yt:xyz123".into()),
            ..Default::default()
        };
        let ids = ext.candidate_ids(&MediaKind::Episode, Some(1), Some(3), Some(&gp));
        assert_eq!(ids[0], "yt:xyz123");
        assert_eq!(ids[1], "tt1844624:1:3");
    }

    #[test]
    fn candidate_ids_episode_no_grandparent_returns_empty() {
        let ext = ExternalIds::default();
        let ids = ext.candidate_ids(&MediaKind::Episode, Some(1), Some(1), None);
        assert!(ids.is_empty());
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

    /// TV episode air dates are digital availability dates in practice. A recently
    /// aired episode without a separate digital date must remain visible, while a
    /// future episode must still be hidden.
    #[tokio::test]
    async fn release_date_filter_uses_episode_air_dates() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let now = chrono::Utc::now().naive_utc();

        let series_ext = ExternalIds {
            imdb: NonEmptyString::try_new("tt_airdate_test".to_string()).ok(),
            ..Default::default()
        };
        let mut series = Media {
            id: uuid::Uuid::from(&MediaIdRaw {
                kind: MediaKind::Series,
                external_ids: series_ext.clone(),
                season: None,
                episode: None,
            }),
            title: "AirDate Test Series".to_string(),
            kind: MediaKind::Series,
            external_ids: series_ext,
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        for (episode_number, title, released_at) in [
            (
                1i64,
                "Recently Aired Episode",
                now - chrono::Duration::days(1),
            ),
            (2, "Future Episode", now + chrono::Duration::days(1)),
        ] {
            let mut episode = Media {
                id: crate::common::stable_media_uuid(
                    &MediaKind::Episode,
                    &format!("air_date_test:{episode_number}"),
                ),
                title: title.to_string(),
                kind: MediaKind::Episode,
                grandparent_id: Some(series.id),
                idx: Some(episode_number),
                parent_idx: Some(1),
                released_at: Some(released_at),
                digital_released_at: None,
                ..Default::default()
            };
            episode
                .save(db)
                .await
                .unwrap();
        }

        let result = Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![MediaKind::Episode]),
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
            titles.contains(&"Recently Aired Episode"),
            "recently aired episode must be visible; got: {:?}",
            titles
        );
        assert!(
            !titles.contains(&"Future Episode"),
            "future episode must remain hidden; got: {:?}",
            titles
        );
    }

    async fn sort_titles(
        db: &sqlx::SqlitePool,
        kind: MediaKind,
        sort_by: api::ItemSortBy,
        order: api::SortOrder,
    ) -> Vec<String> {
        let result = Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![kind]),
                sort_by: vec![sort_by],
                sort_order: vec![order],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        result
            .records
            .into_iter()
            .map(|m| m.title)
            .collect()
    }

    /// Same as `sort_titles` but with a user id so user-data sort arms fire.
    async fn sort_titles_for_user(
        db: &sqlx::SqlitePool,
        kind: MediaKind,
        sort_by: api::ItemSortBy,
        order: api::SortOrder,
        user_id: uuid::Uuid,
    ) -> Vec<String> {
        let result = Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![kind]),
                sort_by: vec![sort_by],
                sort_order: vec![order],
                user_id: Some(user_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        result
            .records
            .into_iter()
            .map(|m| m.title)
            .collect()
    }

    async fn insert_user_state(
        db: &sqlx::SqlitePool,
        user_id: uuid::Uuid,
        media_id: uuid::Uuid,
        play_count: i64,
        favorite: bool,
    ) {
        sqlx::query(
            "INSERT INTO user_media_state \
             (user_id, media_id, favorite, play_count, played_at, playback_position) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        )
        .bind(user_id)
        .bind(media_id)
        .bind(favorite)
        .bind(play_count)
        .bind(
            play_count
                .gt(&0)
                .then(|| chrono::Utc::now().naive_utc()),
        )
        .execute(db)
        .await
        .unwrap();
    }

    fn media_row(kind: MediaKind, title: &str, imdb: &str) -> Media {
        let ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
            ..Default::default()
        };
        let id = uuid::Uuid::from(&MediaIdRaw {
            kind: kind.clone(),
            external_ids: ext.clone(),
            season: None,
            episode: None,
        });
        Media {
            id,
            title: title.to_string(),
            kind,
            external_ids: ext,
            ..Default::default()
        }
    }

    /// CriticRating sorts by rating_critic (descending).
    #[tokio::test]
    async fn sort_by_critic_rating() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let mut low = media_row(MediaKind::Movie, "Low", "tt1001");
        low.rating_critic = Some(10.0);
        let mut mid = media_row(MediaKind::Movie, "Mid", "tt1002");
        mid.rating_critic = Some(50.0);
        let mut high = media_row(MediaKind::Movie, "High", "tt1003");
        high.rating_critic = Some(90.0);
        low.save(db)
            .await
            .unwrap();
        mid.save(db)
            .await
            .unwrap();
        high.save(db)
            .await
            .unwrap();

        let titles = sort_titles(
            db,
            MediaKind::Movie,
            api::ItemSortBy::CriticRating,
            api::SortOrder::Descending,
        )
        .await;
        assert_eq!(titles, vec!["High", "Mid", "Low"]);
    }

    /// OfficialRating sorts by certification_age (descending).
    #[tokio::test]
    async fn sort_by_official_rating() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let mut low = media_row(MediaKind::Movie, "PG13", "tt2001");
        low.certification_age = Some(13);
        let mut mid = media_row(MediaKind::Movie, "PG16", "tt2002");
        mid.certification_age = Some(16);
        let mut high = media_row(MediaKind::Movie, "R18", "tt2003");
        high.certification_age = Some(18);
        low.save(db)
            .await
            .unwrap();
        mid.save(db)
            .await
            .unwrap();
        high.save(db)
            .await
            .unwrap();

        let titles = sort_titles(
            db,
            MediaKind::Movie,
            api::ItemSortBy::OfficialRating,
            api::SortOrder::Descending,
        )
        .await;
        assert_eq!(titles, vec!["R18", "PG16", "PG13"]);
    }

    /// AiredEpisodeOrder sorts by season, then episode number.
    #[tokio::test]
    async fn sort_by_aired_episode_order() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let series_ext = ExternalIds {
            imdb: NonEmptyString::try_new("tt_sort_ep_test".to_string()).ok(),
            ..Default::default()
        };
        let mut series = Media {
            id: uuid::Uuid::from(&MediaIdRaw {
                kind: MediaKind::Series,
                external_ids: series_ext.clone(),
                season: None,
                episode: None,
            }),
            title: "Sort Episode Test Series".to_string(),
            kind: MediaKind::Series,
            external_ids: series_ext,
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        for (t, s, e) in [
            ("S1E2", 1i64, 2i64),
            ("S1E1", 1, 1),
            ("S2E1", 2, 1),
            ("S0E5", 0, 5),
        ] {
            let mut ep = Media {
                id: crate::common::stable_media_uuid(
                    &MediaKind::Episode,
                    &format!("sort_ep_test:{s}:{e}"),
                ),
                title: t.to_string(),
                kind: MediaKind::Episode,
                grandparent_id: Some(series.id),
                ..Default::default()
            };
            ep.parent_idx = Some(s);
            ep.idx = Some(e);
            ep.save(db)
                .await
                .unwrap();
        }

        let titles = sort_titles(
            db,
            MediaKind::Episode,
            api::ItemSortBy::AiredEpisodeOrder,
            api::SortOrder::Ascending,
        )
        .await;
        assert_eq!(titles, vec!["S0E5", "S1E1", "S1E2", "S2E1"]);
    }

    /// Artist sorts by the grandparent (artist) row title; Album by the parent
    /// (album) row title.
    #[tokio::test]
    async fn sort_by_artist_and_album() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;

        // Build with the deezer ids set BEFORE deriving the stable UUID, so each
        // row gets a distinct id (canonical() for music uses only the deezer id).
        let music = |kind: MediaKind, title: &str, imdb: &str, deezer: i64| {
            let mut ext = ExternalIds {
                imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
                ..Default::default()
            };
            match kind {
                MediaKind::Artist => ext.deezer_artist = Some(deezer),
                MediaKind::Album => ext.deezer_album = Some(deezer),
                MediaKind::Track => ext.deezer_track = Some(deezer),
                _ => {}
            }
            let id = uuid::Uuid::from(&MediaIdRaw {
                kind: kind.clone(),
                external_ids: ext.clone(),
                season: None,
                episode: None,
            });
            Media {
                id,
                title: title.to_string(),
                kind,
                external_ids: ext,
                ..Default::default()
            }
        };

        let mut artist_a = music(MediaKind::Artist, "Adele", "tt4001", 1);
        artist_a
            .save(db)
            .await
            .unwrap();
        let mut artist_z = music(MediaKind::Artist, "Zed", "tt4002", 2);
        artist_z
            .save(db)
            .await
            .unwrap();

        let mut album_21 = music(MediaKind::Album, "21", "tt4003", 3);
        album_21.parent_id = Some(artist_a.id);
        album_21
            .save(db)
            .await
            .unwrap();
        let mut album_z = music(MediaKind::Album, "AlbumZ", "tt4004", 4);
        album_z.parent_id = Some(artist_z.id);
        album_z
            .save(db)
            .await
            .unwrap();

        let mut t1 = music(MediaKind::Track, "Hello", "tt4005", 5);
        t1.parent_id = Some(album_21.id);
        t1.grandparent_id = Some(artist_a.id);
        t1.save(db)
            .await
            .unwrap();
        let mut t2 = music(MediaKind::Track, "Zed Song", "tt4006", 6);
        t2.parent_id = Some(album_z.id);
        t2.grandparent_id = Some(artist_z.id);
        t2.save(db)
            .await
            .unwrap();

        let by_artist = sort_titles(
            db,
            MediaKind::Track,
            api::ItemSortBy::Artist,
            api::SortOrder::Ascending,
        )
        .await;
        assert_eq!(by_artist, vec!["Hello", "Zed Song"]);

        let by_album = sort_titles(
            db,
            MediaKind::Track,
            api::ItemSortBy::Album,
            api::SortOrder::Ascending,
        )
        .await;
        assert_eq!(by_album, vec!["Hello", "Zed Song"]);
    }

    /// PlayCount / IsPlayed / IsUnplayed / IsFavoriteOrLiked sort via correlated
    /// user_media_state lookups and never exclude unplayed items.
    #[tokio::test]
    async fn sort_by_user_data_state() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let mut played = media_row(MediaKind::Movie, "Played", "tt6001");
        played
            .save(db)
            .await
            .unwrap();
        let mut unplayed = media_row(MediaKind::Movie, "Unplayed", "tt6002");
        unplayed
            .save(db)
            .await
            .unwrap();
        let mut favorite = media_row(MediaKind::Movie, "Favorite", "tt6003");
        favorite
            .save(db)
            .await
            .unwrap();

        insert_user_state(db, uid, played.id, 3, false).await;
        insert_user_state(db, uid, favorite.id, 0, true).await;
        // unplayed has NO state row — must still appear.

        let by_count = sort_titles_for_user(
            db,
            MediaKind::Movie,
            api::ItemSortBy::PlayCount,
            api::SortOrder::Descending,
            uid,
        )
        .await;
        assert_eq!(by_count[0], "Played");
        assert_eq!(by_count.len(), 3);

        let played_first = sort_titles_for_user(
            db,
            MediaKind::Movie,
            api::ItemSortBy::IsPlayed,
            api::SortOrder::Ascending,
            uid,
        )
        .await;
        assert_eq!(played_first[0], "Played");
        assert!(
            played_first
                .iter()
                .any(|t| t == "Unplayed")
        );

        let unplayed_first = sort_titles_for_user(
            db,
            MediaKind::Movie,
            api::ItemSortBy::IsUnplayed,
            api::SortOrder::Ascending,
            uid,
        )
        .await;
        assert_eq!(unplayed_first[0], "Unplayed");
        assert!(
            unplayed_first
                .iter()
                .any(|t| t == "Played")
        );

        let favs = sort_titles_for_user(
            db,
            MediaKind::Movie,
            api::ItemSortBy::IsFavoriteOrLiked,
            api::SortOrder::Descending,
            uid,
        )
        .await;
        assert_eq!(favs[0], "Favorite");
    }

    /// SeriesSortName sorts episodes by their series (grandparent) title.
    #[tokio::test]
    async fn sort_by_series_sort_name() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;

        let mut series_a = media_row(MediaKind::Series, "Alpha Show", "tt7001");
        series_a
            .save(db)
            .await
            .unwrap();
        let mut series_z = media_row(MediaKind::Series, "Zulu Show", "tt7002");
        series_z
            .save(db)
            .await
            .unwrap();

        let mk_episode = |title: &str, gp: uuid::Uuid| {
            let mut ep = Media {
                id: uuid::Uuid::new_v4(),
                title: title.to_string(),
                kind: MediaKind::Episode,
                ..Default::default()
            };
            ep.grandparent_id = Some(gp);
            ep.parent_idx = Some(1);
            ep.idx = Some(1);
            ep
        };
        let mut ep_a = mk_episode("Alpha Ep", series_a.id);
        ep_a.save(db)
            .await
            .unwrap();
        let mut ep_z = mk_episode("Zulu Ep", series_z.id);
        ep_z.save(db)
            .await
            .unwrap();

        let titles = sort_titles(
            db,
            MediaKind::Episode,
            api::ItemSortBy::SeriesSortName,
            api::SortOrder::Ascending,
        )
        .await;
        assert_eq!(titles, vec!["Alpha Ep", "Zulu Ep"]);
    }

    /// DateLastContentAdded sorts series by their most recently added episode.
    #[tokio::test]
    async fn sort_by_date_last_content_added() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let now = chrono::Utc::now().naive_utc();

        let mut series_old = media_row(MediaKind::Series, "Old Series", "tt8001");
        series_old.created_at = now - chrono::Duration::days(100);
        series_old
            .save(db)
            .await
            .unwrap();
        let mut series_new = media_row(MediaKind::Series, "New Series", "tt8002");
        series_new.created_at = now - chrono::Duration::days(100);
        series_new
            .save(db)
            .await
            .unwrap();

        let mk_episode =
            |title: &str, gp: uuid::Uuid, created: chrono::NaiveDateTime| {
                let mut ep = Media {
                    id: uuid::Uuid::new_v4(),
                    title: title.to_string(),
                    kind: MediaKind::Episode,
                    created_at: created,
                    ..Default::default()
                };
                ep.grandparent_id = Some(gp);
                ep.parent_idx = Some(1);
                ep.idx = Some(1);
                ep
            };
        let mut ep_old =
            mk_episode("Old Ep", series_old.id, now - chrono::Duration::days(50));
        ep_old
            .save(db)
            .await
            .unwrap();
        let mut ep_new =
            mk_episode("New Ep", series_new.id, now - chrono::Duration::days(1));
        ep_new
            .save(db)
            .await
            .unwrap();

        let titles = sort_titles(
            db,
            MediaKind::Series,
            api::ItemSortBy::DateLastContentAdded,
            api::SortOrder::Descending,
        )
        .await;
        assert_eq!(titles, vec!["New Series", "Old Series"]);
    }

    /// The Albums view excludes Deezer singles/EPs but keeps albums (including
    /// albums without a stored type).
    #[tokio::test]
    async fn album_kinds_filters_release_kinds() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;

        let album_row =
            |title: &str, imdb: &str, deezer: i64, kind: Option<AlbumKind>| {
                let mut ext = ExternalIds {
                    imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
                    deezer_album: Some(deezer),
                    ..Default::default()
                };
                let id = uuid::Uuid::from(&MediaIdRaw {
                    kind: MediaKind::Album,
                    external_ids: ext.clone(),
                    season: None,
                    episode: None,
                });
                Media {
                    id,
                    title: title.to_string(),
                    kind: MediaKind::Album,
                    album_kind: kind,
                    external_ids: ext,
                    ..Default::default()
                }
            };

        let mut album = album_row("Real Album", "tt9001", 1, Some(AlbumKind::Album));
        album
            .save(db)
            .await
            .unwrap();
        let mut single = album_row("Single", "tt9002", 2, Some(AlbumKind::Single));
        single
            .save(db)
            .await
            .unwrap();
        let mut ep = album_row("EP", "tt9003", 3, Some(AlbumKind::Ep));
        ep.save(db)
            .await
            .unwrap();
        let mut no_type = album_row("No Type", "tt9004", 4, None);
        no_type
            .save(db)
            .await
            .unwrap();

        let fetch_titles = |album_kinds: Option<Vec<AlbumKind>>| async move {
            let result = Media::get_by_filter(
                db,
                &MediaFilter {
                    kind: Some(vec![MediaKind::Album]),
                    album_kinds,
                    sort_by: vec![api::ItemSortBy::SortName],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            result
                .records
                .into_iter()
                .map(|m| m.title)
                .collect::<Vec<_>>()
        };

        let all = fetch_titles(None).await;
        assert_eq!(
            all.len(),
            4,
            "without the filter every album is returned; got {all:?}"
        );

        let filtered = fetch_titles(Some(vec![AlbumKind::Album])).await;
        assert_eq!(filtered, vec!["No Type", "Real Album"]);
    }

    mod widen_external_ids {
        use super::*;
        use crate::integration_test::new_test_server;

        async fn seed(ctx: &crate::AppContext, ids: ExternalIds) -> Uuid {
            let mut m = Media {
                id: Uuid::from(&MediaIdRaw {
                    kind: MediaKind::Movie,
                    external_ids: ids.clone(),
                    season: None,
                    episode: None,
                }),
                title: "Heat".into(),
                kind: MediaKind::Movie,
                external_ids: ids,
                ..Default::default()
            };
            m.save(&ctx.db)
                .await
                .unwrap();
            m.id
        }

        async fn stored(ctx: &crate::AppContext, id: &Uuid) -> ExternalIds {
            Media::get_by_id(&ctx.db, id)
                .await
                .unwrap()
                .unwrap()
                .external_ids
        }

        /// Two lookups read the same row and resolve different ids; the second
        /// write used to carry a snapshot that predated the first.
        #[tokio::test]
        async fn a_stale_writer_cannot_erase_what_another_added() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let id = seed(
                ctx,
                ExternalIds {
                    imdb: NonEmptyString::try_new("tt0113277".to_string()).ok(),
                    ..Default::default()
                },
            )
            .await;

            Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tmdb: Some(949),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tvdb: Some(468),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            let ids = stored(ctx, &id).await;
            assert_eq!(ids.tmdb, Some(949), "the earlier write was erased");
            assert_eq!(ids.tvdb, Some(468));
            assert_eq!(
                ids.imdb
                    .map(String::from),
                Some("tt0113277".to_string()),
                "the seeded id was erased"
            );
        }

        #[tokio::test]
        async fn a_stored_id_wins_over_the_patch() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let id = seed(
                ctx,
                ExternalIds {
                    imdb: NonEmptyString::try_new("tt0113277".to_string()).ok(),
                    tmdb: Some(949),
                    ..Default::default()
                },
            )
            .await;

            let merged = Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tmdb: Some(1111),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();

            assert_eq!(merged.tmdb, Some(949));
            assert_eq!(
                stored(ctx, &id)
                    .await
                    .tmdb,
                Some(949)
            );
        }

        /// An episode the caller never saved must not fail its delivery.
        #[tokio::test]
        async fn a_row_that_does_not_exist_is_not_an_error() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;

            assert!(
                Media::widen_external_ids(
                    &ctx.db,
                    &Uuid::new_v4(),
                    &ExternalIds {
                        tmdb: Some(949),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .is_none()
            );
        }

        /// The other half of the race: a refresh that loaded the row before an
        /// enrichment must not upsert its stale snapshot over the new ids.
        #[tokio::test]
        async fn a_save_does_not_drop_ids_resolved_since_it_loaded() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let ids = ExternalIds {
                imdb: NonEmptyString::try_new("tt0113277".to_string()).ok(),
                ..Default::default()
            };
            let id = seed(ctx, ids.clone()).await;

            let mut stale = Media::get_by_id(&ctx.db, &id)
                .await
                .unwrap()
                .unwrap();
            Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tmdb: Some(949),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            stale.title = "Heat (1995)".into();
            stale
                .save(&ctx.db)
                .await
                .unwrap();

            let after = stored(ctx, &id).await;
            assert_eq!(after.tmdb, Some(949), "the stale save dropped it");
            assert_eq!(
                after
                    .imdb
                    .map(String::from),
                Some("tt0113277".to_string())
            );
        }

        /// A row written before `skip_serializing_none` can hold an explicit
        /// null, which as a merge patch would delete rather than keep.
        #[tokio::test]
        async fn a_stored_null_does_not_delete_the_id_being_added() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let id = seed(
                ctx,
                ExternalIds {
                    imdb: NonEmptyString::try_new("tt0113277".to_string()).ok(),
                    ..Default::default()
                },
            )
            .await;
            sqlx::query(
                "UPDATE media \
                 SET external_ids = json('{\"imdb\":\"tt0113277\",\"tmdb\":null}') \
                 WHERE id = ?1",
            )
            .bind(id)
            .execute(&ctx.db)
            .await
            .unwrap();

            let merged = Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tmdb: Some(949),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();

            assert_eq!(merged.tmdb, Some(949));
            assert_eq!(
                stored(ctx, &id)
                    .await
                    .tmdb,
                Some(949)
            );
        }

        /// `external_ids` has been found holding `''`, which `sqlx` cannot decode.
        #[tokio::test]
        async fn a_row_holding_invalid_json_is_repaired() {
            let (_s, guard) = new_test_server()
                .await
                .unwrap();
            let ctx = &guard.0;
            let id = seed(
                ctx,
                ExternalIds {
                    imdb: NonEmptyString::try_new("tt0113277".to_string()).ok(),
                    ..Default::default()
                },
            )
            .await;
            sqlx::query("UPDATE media SET external_ids = '' WHERE id = ?1")
                .bind(id)
                .execute(&ctx.db)
                .await
                .unwrap();

            let merged = Media::widen_external_ids(
                &ctx.db,
                &id,
                &ExternalIds {
                    tmdb: Some(949),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();

            assert_eq!(merged.tmdb, Some(949));
            assert_eq!(
                stored(ctx, &id)
                    .await
                    .tmdb,
                Some(949)
            );
        }
    }
    async fn filter_rule_titles(
        db: &sqlx::SqlitePool,
        rule: remux_sdks::remux::FilterRule,
        user_id: uuid::Uuid,
    ) -> Vec<String> {
        let filter = remux_sdks::remux::CollectionFilter {
            groups: vec![remux_sdks::remux::FilterGroup {
                rules: vec![rule],
                match_mode: remux_sdks::remux::FilterMatchMode::All,
            }],
            match_mode: remux_sdks::remux::FilterMatchMode::All,
        };
        Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![MediaKind::Movie]),
                filter_rules: Some(filter),
                user_id: Some(user_id),
                sort_by: vec![api::ItemSortBy::SortName],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|m| m.title)
        .collect()
    }

    #[tokio::test]
    async fn favorite_filter_rule_returns_only_favorited_items() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let mut fav = media_row(MediaKind::Movie, "Fav", "tt8880001");
        fav.save(db)
            .await
            .unwrap();
        let mut not_fav = media_row(MediaKind::Movie, "NotFav", "tt8880002");
        not_fav
            .save(db)
            .await
            .unwrap();
        // no_state has no UMS row at all
        let mut no_state = media_row(MediaKind::Movie, "NoState", "tt8880003");
        no_state
            .save(db)
            .await
            .unwrap();

        insert_user_state(db, uid, fav.id, 0, true).await;
        insert_user_state(db, uid, not_fav.id, 0, false).await;

        let titles = filter_rule_titles(
            db,
            remux_sdks::remux::FilterRule::Favorite { value: true },
            uid,
        )
        .await;
        assert_eq!(titles, vec!["Fav"]);
    }

    #[tokio::test]
    async fn favorite_filter_rule_not_favorite_excludes_explicit_and_absent_favorites()
    {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let mut fav = media_row(MediaKind::Movie, "Fav", "tt8881001");
        fav.save(db)
            .await
            .unwrap();
        let mut not_fav = media_row(MediaKind::Movie, "NotFav", "tt8881002");
        not_fav
            .save(db)
            .await
            .unwrap();
        let mut no_state = media_row(MediaKind::Movie, "NoState", "tt8881003");
        no_state
            .save(db)
            .await
            .unwrap();

        insert_user_state(db, uid, fav.id, 0, true).await;
        insert_user_state(db, uid, not_fav.id, 0, false).await;

        let mut titles = filter_rule_titles(
            db,
            remux_sdks::remux::FilterRule::Favorite { value: false },
            uid,
        )
        .await;
        titles.sort();
        assert_eq!(titles, vec!["NoState", "NotFav"]);
    }

    #[tokio::test]
    async fn favorite_filter_rule_is_scoped_to_the_requesting_user() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();

        let mut item = media_row(MediaKind::Movie, "Item", "tt8882001");
        item.save(db)
            .await
            .unwrap();

        // only the other user has favorited this item
        insert_user_state(db, other, item.id, 0, true).await;

        let titles = filter_rule_titles(
            db,
            remux_sdks::remux::FilterRule::Favorite { value: true },
            uid,
        )
        .await;
        assert!(
            titles.is_empty(),
            "another user's favorite must not bleed through"
        );
    }

    // ── Watched filter rule tests ─────────────────────────────────────

    async fn watched_filter(
        db: &sqlx::SqlitePool,
        kind: MediaKind,
        user_id: uuid::Uuid,
        sort_by: Vec<api::ItemSortBy>,
    ) -> Vec<String> {
        let filter = remux_sdks::remux::CollectionFilter {
            groups: vec![remux_sdks::remux::FilterGroup {
                rules: vec![remux_sdks::remux::FilterRule::Played { value: true }],
                match_mode: remux_sdks::remux::FilterMatchMode::All,
            }],
            match_mode: remux_sdks::remux::FilterMatchMode::All,
        };
        Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![kind]),
                filter_rules: Some(filter),
                user_id: Some(user_id),
                sort_by,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .records
        .into_iter()
        .map(|m| m.title)
        .collect()
    }

    /// Watched=true + DatePlayed sort for a user with no watched items must
    /// return an empty list without panicking or returning a 500 (was: invalid
    /// SQL "CASE media.id ELSE ... END" when the rollup is empty).
    #[tokio::test]
    async fn watched_empty_user_no_crash() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let mut m = media_row(MediaKind::Movie, "Unwatched", "tt9990101");
        m.save(db)
            .await
            .unwrap();

        let titles = watched_filter(
            db,
            MediaKind::Movie,
            uid,
            vec![api::ItemSortBy::DatePlayed],
        )
        .await;

        assert!(
            titles.is_empty(),
            "no watched items → empty result, not a crash"
        );
    }

    /// Watched=true returns only movies with play_count > 0 for the requesting
    /// user; unwatched movies and another user's watched movies are excluded.
    #[tokio::test]
    async fn watched_filter_returns_watched_movies() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();

        let mut a = media_row(MediaKind::Movie, "Alpha", "tt9990201");
        a.save(db)
            .await
            .unwrap();
        let mut b = media_row(MediaKind::Movie, "Beta", "tt9990202");
        b.save(db)
            .await
            .unwrap();
        let mut c = media_row(MediaKind::Movie, "Gamma", "tt9990203");
        c.save(db)
            .await
            .unwrap();

        insert_user_state(db, uid, a.id, 1, false).await;
        insert_user_state(db, uid, b.id, 0, false).await; // not watched
        insert_user_state(db, other, c.id, 1, false).await; // different user

        let mut titles =
            watched_filter(db, MediaKind::Movie, uid, vec![api::ItemSortBy::SortName])
                .await;
        titles.sort();

        assert_eq!(titles, vec!["Alpha"], "only uid's watched movies");
    }

    /// A watched episode must cause its grandparent series to appear when
    /// querying with Watched=true on the Series kind.
    #[tokio::test]
    async fn watched_episode_rolls_up_to_series() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let mut series = media_row(MediaKind::Series, "Show", "tt9990301");
        series
            .save(db)
            .await
            .unwrap();

        let ep_ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new("tt9990302".to_string()).unwrap()),
            ..Default::default()
        };
        let ep_id = uuid::Uuid::from(&MediaIdRaw {
            kind: MediaKind::Episode,
            external_ids: ep_ext.clone(),
            season: Some(1),
            episode: Some(1),
        });
        let mut episode = Media {
            id: ep_id,
            title: "Pilot".to_string(),
            kind: MediaKind::Episode,
            external_ids: ep_ext,
            grandparent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        episode
            .save(db)
            .await
            .unwrap();

        // Only the episode is watched — not the series row itself.
        insert_user_state(db, uid, episode.id, 1, false).await;

        let titles = watched_filter(
            db,
            MediaKind::Series,
            uid,
            vec![api::ItemSortBy::DatePlayed],
        )
        .await;

        assert_eq!(
            titles,
            vec!["Show"],
            "watched episode must roll up to series"
        );
    }

    /// Watched=true + DatePlayed sort must complete well within 1 s even with
    /// a large number of media rows and UMS rows. Run with:
    ///   cargo test -p remux-server watched_date_played_perf -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "perf test — run manually to compare before/after"]
    async fn watched_date_played_perf() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let uid = uuid::Uuid::new_v4();

        let n_total: usize = 30_000;
        let n_watched: usize = 3_000;
        let page_size: u32 = 50; // realistic client page size
        let mut ids = Vec::with_capacity(n_total);
        for i in 0..n_total {
            let imdb = format!("tt{:07}", 7_000_000 + i);
            let mut m = media_row(MediaKind::Movie, &format!("Movie {i}"), &imdb);
            m.save(db)
                .await
                .unwrap();
            ids.push(m.id);
        }
        for id in ids
            .iter()
            .take(n_watched)
        {
            insert_user_state(db, uid, *id, 1, false).await;
        }

        let filter_rules = remux_sdks::remux::CollectionFilter {
            groups: vec![remux_sdks::remux::FilterGroup {
                rules: vec![remux_sdks::remux::FilterRule::Played { value: true }],
                match_mode: remux_sdks::remux::FilterMatchMode::All,
            }],
            match_mode: remux_sdks::remux::FilterMatchMode::All,
        };

        let start = std::time::Instant::now();
        let result = Media::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(vec![MediaKind::Movie]),
                filter_rules: Some(filter_rules),
                user_id: Some(uid),
                sort_by: vec![api::ItemSortBy::DatePlayed],
                limit: Some(page_size),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(
            result
                .records
                .len(),
            page_size as usize,
            "must return exactly limit rows"
        );
        println!(
            "watched DatePlayed query over {n_total} rows (page {page_size}): {elapsed:?}"
        );
        assert!(
            elapsed.as_millis() < 150,
            "query took {elapsed:?}, expected < 150 ms — correlated subquery regression?"
        );
    }
}
