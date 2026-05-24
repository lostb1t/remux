use super::{FilterResult, ImageKind, MediaImage, MediaImages, QueryBuilderExt};
use crate::api;
use crate::api::MediaSourceInfo;
use crate::common::IntoVec;
use crate::common::get_uuid;
use crate::common::server_id;
use crate::sdks;
use crate::services::stremio as stremio_service;
use crate::stream::{StreamDescriptor, StreamInfo};
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use axum::ServiceExt;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::middleware;
use axum::middleware::Next;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::{
    Json, Router,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
};
use axum_anyhow::ApiError;
use axum_anyhow::on_error;
use axum_anyhow::set_expose_errors;
use axum_anyhow::{ApiResult, OptionExt, ResultExt};
use chrono::prelude::*;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use config;
use config::Config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use regex::Regex;
use reqwest;
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_with::skip_serializing_none;
use sqlx::Row;
use sqlx::SqlitePool;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use timed;
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing;
use tracing::debug;
use tracing::info;
use tracing::instrument;
use tracing::trace;
use tracing::warn;
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
            sdks::stremio::MediaType::Series | sdks::stremio::MediaType::Tv => {
                Ok(MediaKind::Series)
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
            MediaKind::Genre => sdks::remux::MediaKind::Genre,
            MediaKind::Person => sdks::remux::MediaKind::Person,
            MediaKind::Studio => sdks::remux::MediaKind::Studio,
            MediaKind::Stream => sdks::remux::MediaKind::Stream,
            MediaKind::TvChannel => sdks::remux::MediaKind::TvChannel,
            MediaKind::TvProgram => sdks::remux::MediaKind::TvProgram,
            MediaKind::Track => sdks::remux::MediaKind::Track,
            MediaKind::Album => sdks::remux::MediaKind::Album,
            MediaKind::Artist => sdks::remux::MediaKind::Artist,
            MediaKind::Playlist => sdks::remux::MediaKind::Playlist,
            MediaKind::StreamGroup => sdks::remux::MediaKind::Stream,
        }
    }
}

impl From<sdks::remux::MediaKind> for MediaKind {
    fn from(k: sdks::remux::MediaKind) -> Self {
        match k {
            sdks::remux::MediaKind::Movie => MediaKind::Movie,
            sdks::remux::MediaKind::Series => MediaKind::Series,
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
            api::MediaType::Genre | api::MediaType::MusicGenre => Ok(MediaKind::Genre),
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
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum CollectionMediaKind {
    #[default]
    Movie,
    Series,
    Music,
    Collection,
}

impl TryFrom<String> for CollectionMediaKind {
    type Error = strum::ParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
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

        let mut tx = db.begin().await?;
        const BATCH_SIZE: usize = 500;

        for chunk in items.chunks(BATCH_SIZE) {
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

            qb.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
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
        const CHUNK: usize = 999;
        for chunk in ids.chunks(CHUNK) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media_relations WHERE left_media_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            qb.build().execute(db).await?;
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
        let mut next_weight = max_weight.map(|w| w + 1).unwrap_or(0);
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
        Self::upsert(db, &items).await
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
        qb.build().execute(db).await?;
        Ok(())
    }

    pub async fn move_playlist_item(
        db: &SqlitePool,
        playlist_id: &Uuid,
        relation_id: &Uuid,
        new_index: usize,
    ) -> Result<()> {
        let mut items = Self::get_playlist_items(db, playlist_id).await?;
        let Some(pos) = items.iter().position(|r| &r.relation_id == relation_id) else {
            return Ok(());
        };
        let item = items.remove(pos);
        let insert_at = new_index.min(items.len());
        items.insert(insert_at, item);

        let mut tx = db.begin().await?;
        for (i, r) in items.iter().enumerate() {
            sqlx::query("UPDATE media_relations SET weight = ? WHERE relation_id = ?")
                .bind(i as i64)
                .bind(r.relation_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalIds {
    pub imdb: Option<String>,
    pub series_imdb: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub deezer_artist: Option<i64>,
    pub deezer_album: Option<i64>,
    pub deezer_track: Option<i64>,
    pub youtube_id: Option<String>,
    pub iptv_source_id: Option<String>,
    pub iptv_group: Option<String>,
}

impl ExternalIds {
    /// Parse an AIO `meta.id` string into external provider IDs using the
    /// standard Stremio/Jellyfin prefix conventions.
    pub fn from_stremio_id(id: &str) -> Self {
        if id.starts_with("tt") {
            return Self {
                imdb: Some(id.to_string()),
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
            let value = cap[2].trim().to_string();
            match provider.as_str() {
                "tmdb" | "tmdbid" => {
                    if result.tmdb.is_none() {
                        result.tmdb = value.parse::<i64>().ok();
                    }
                }
                "imdb" | "imdbid" => {
                    if result.imdb.is_none() {
                        result.imdb = Some(value);
                    }
                }
                "tvdb" | "tvdbid" => {
                    if result.tvdb.is_none() {
                        result.tvdb = value.parse::<i64>().ok();
                    }
                }
                _ => {}
            }
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        self.imdb.is_none()
            && self.series_imdb.is_none()
            && self.tmdb.is_none()
            && self.tvdb.is_none()
    }

    /// Merge another `ExternalIds` into `self`, with `other` taking precedence
    /// for any field that is `Some`.
    pub fn merge(mut self, other: Self) -> Self {
        if other.imdb.is_some() {
            self.imdb = other.imdb;
        }
        if other.series_imdb.is_some() {
            self.series_imdb = other.series_imdb;
        }
        if other.tmdb.is_some() {
            self.tmdb = other.tmdb;
        }
        if other.tvdb.is_some() {
            self.tvdb = other.tvdb;
        }
        self
    }
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
    /// If set, only return items where COALESCE(digital_released_at, released_at) <= threshold.
    pub digital_released_before: Option<NaiveDateTime>,
    /// Sort order for results. Mapped from Jellyfin's ItemSortBy.
    pub sort_by: Vec<api::ItemSortBy>,
    pub sort_order: Vec<api::SortOrder>,
    /// For TvProgram queries: order by the parent channel's sort_order / channel_number.
    pub sort_by_channel_order: bool,
    /// Structured filter rules (from smart collections). Evaluated with `filter_match`.
    pub filter_rules: Vec<remux_sdks::remux::FilterRule>,
    /// Whether all rules must match (AND) or any rule (OR). Defaults to All.
    pub filter_match: remux_sdks::remux::FilterMatchMode,
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
                .find(|cc| cc.name.eq_ignore_ascii_case(c))
                .copied()
        })
        .map(|cc| cc.alpha2.to_string())
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
    #[sqlx(skip)]
    pub images: MediaImages,
    pub status: Option<MediaStatus>,
    pub idx: Option<i64>,
    pub parent_idx: Option<i64>,
    pub parent_id: Option<Uuid>,
    #[sqlx(default)]
    #[sqlx(json)]
    pub external_ids: ExternalIds,
    pub grandparent_id: Option<Uuid>,
    //pub season_id: Option<Uuid>,
    //pub description: Option<String>,
    #[sqlx(skip)]
    pub tags: Vec<String>,
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
    /// Season/album title (parent item's title), populated post-query.
    #[sqlx(skip)]
    pub parent_title: Option<String>,
    /// Series/artist title (series item's title), populated post-query.
    #[sqlx(skip)]
    pub series_title: Option<String>,
    /// Series poster hash for episodes/seasons, populated post-query.
    #[sqlx(skip)]
    pub series_poster: Option<String>,
    /// Series backdrop hash for episodes/seasons, populated post-query.
    #[sqlx(skip)]
    pub series_backdrop: Option<String>,
    /// Series thumb hash for episodes/seasons, populated post-query.
    #[sqlx(skip)]
    pub series_thumb: Option<String>,
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
    // CollectionMediaKind
    pub collection_media_kind: Option<CollectionMediaKind>,
    pub collection_max_items: Option<i64>,
    #[sqlx(json(nullable))]
    pub collection_smart_filter: Option<remux_sdks::remux::CollectionFilter>,

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
    /// Batch-populate parent/series title fields for tracks, albums, episodes, seasons.
    pub async fn enrich_parents(db: &SqlitePool, records: &mut Vec<Self>) {
        struct ParentRow {
            title: String,
            channel_number: Option<i64>,
        }

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
            .flat_map(|m| [m.parent_id, m.grandparent_id].into_iter().flatten())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        if ids_needed.is_empty() {
            return;
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
            if let Ok(rows) = qb.build().fetch_all(db).await {
                parent_map.extend(rows.into_iter().filter_map(|r| {
                    let id: Option<Uuid> = r.get(0);
                    let title: Option<String> = r.get(1);
                    let channel_number: Option<i64> = r.get(2);
                    id.zip(title).map(|(id, title)| {
                        (
                            id,
                            ParentRow {
                                title,
                                channel_number,
                            },
                        )
                    })
                }));
            }
        }

        if parent_map.is_empty() {
            return;
        }

        // Batch-load images for parent series/season items.
        let mut parent_images =
            super::image::MediaImage::get_for_media_ids(db, &ids_needed)
                .await
                .unwrap_or_default();

        for media in records.iter_mut() {
            match media.kind {
                MediaKind::Track => {
                    media.parent_title = media
                        .parent_id
                        .and_then(|id| parent_map.get(&id).map(|r| r.title.clone()));
                    media.series_title = media
                        .grandparent_id
                        .and_then(|id| parent_map.get(&id).map(|r| r.title.clone()));
                }
                MediaKind::Album => {
                    media.series_title = media
                        .grandparent_id
                        .and_then(|id| parent_map.get(&id).map(|r| r.title.clone()));
                }
                MediaKind::Episode => {
                    media.parent_title = media
                        .parent_id
                        .and_then(|id| parent_map.get(&id).map(|r| r.title.clone()));
                    let series_id = media.grandparent_id.or(media.parent_id);
                    if let Some(id) = series_id {
                        if let Some(row) = parent_map.get(&id) {
                            media.series_title = Some(row.title.clone());
                        }
                        if let Some(imgs) = parent_images.get(&id) {
                            media.series_poster = imgs
                                .get(super::image::ImageKind::Primary)
                                .map(|i| i.id.to_string());
                            media.series_backdrop = imgs
                                .get(super::image::ImageKind::Backdrop)
                                .map(|i| i.id.to_string());
                            media.series_thumb = imgs
                                .get(super::image::ImageKind::Thumb)
                                .map(|i| i.id.to_string());
                        }
                    }
                }
                MediaKind::Season => {
                    if let Some(id) = media.parent_id {
                        if let Some(row) = parent_map.get(&id) {
                            media.series_title = Some(row.title.clone());
                        }
                        if let Some(imgs) = parent_images.get(&id) {
                            media.series_poster = imgs
                                .get(super::image::ImageKind::Primary)
                                .map(|i| i.id.to_string());
                            media.series_backdrop = imgs
                                .get(super::image::ImageKind::Backdrop)
                                .map(|i| i.id.to_string());
                            media.series_thumb = imgs
                                .get(super::image::ImageKind::Thumb)
                                .map(|i| i.id.to_string());
                        }
                    }
                }
                MediaKind::TvProgram => {
                    if let Some(id) = media.parent_id {
                        if let Some(row) = parent_map.get(&id) {
                            media.parent_title = Some(row.title.clone());
                            media.channel_number = row.channel_number;
                        }
                        if let Some(imgs) = parent_images.get(&id) {
                            media.series_poster = imgs
                                .get(super::image::ImageKind::Primary)
                                .map(|i| i.id.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn parse_smart_filter(&self) -> Option<&remux_sdks::remux::CollectionFilter> {
        self.collection_smart_filter.as_ref()
    }

    pub fn is_remote_url(&self) -> bool {
        matches!(
            self.stream_info.as_ref().map(|si| &si.descriptor),
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
            kind: self.kind.clone(),
            external_ids: self.external_ids.clone(),
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
        self.images.get_path(kind)
    }

    pub fn set_image(&mut self, kind: ImageKind, url: String) {
        let media_id = self.id;
        let vec = match kind {
            ImageKind::Primary => &mut self.images.primary,
            ImageKind::Backdrop => &mut self.images.backdrop,
            ImageKind::Logo => &mut self.images.logo,
            ImageKind::Thumb => &mut self.images.thumb,
        };
        if let Some(existing) = vec.iter_mut().find(|i| i.image_index == 0) {
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
            && self.idx.is_none()
        {
            return Err(MediaError::ValidationError(format!(
                "{:?} requires an index number",
                self.kind
            )));
        }

        let missing = match self.kind {
            MediaKind::Movie | MediaKind::Series => {
                self.external_ids.imdb.is_none().then_some("imdb")
            }
            MediaKind::Season | MediaKind::Episode => self
                .external_ids
                .series_imdb
                .is_none()
                .then_some("series_imdb"),
            MediaKind::Artist => self
                .external_ids
                .deezer_artist
                .is_none()
                .then_some("deezer_artist"),
            MediaKind::Album => (self.external_ids.deezer_album.is_none()
                && self.external_ids.youtube_id.is_none())
            .then_some("deezer_album or youtube_id"),
            MediaKind::Track => (self.external_ids.deezer_track.is_none()
                && self.external_ids.youtube_id.is_none())
            .then_some("deezer_track or youtube_id"),
            _ => None,
        };

        if let Some(field) = missing {
            return Err(MediaError::ValidationError(format!(
                "{:?} requires {field}",
                self.kind
            )));
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
            external_ids, created_at, updated_at, certification, certification_age, parent_idx,
            live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, refreshed_at, grandparent_id,
            collection_smart_filter, country, program_kind
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37)
        ON CONFLICT (id) DO UPDATE SET
            title = excluded.title,
            kind = excluded.kind,
            idx = excluded.idx,
            released_at = excluded.released_at,
            digital_released_at = excluded.digital_released_at,
            runtime = excluded.runtime,
            rating_critic = excluded.rating_critic,
            rating_audience = excluded.rating_audience,
            description = excluded.description,
            trailers = excluded.trailers,
            stream_info = excluded.stream_info,
            probe_data = CASE
                WHEN excluded.stream_info IS NOT media.stream_info THEN NULL
                ELSE COALESCE(excluded.probe_data, media.probe_data)
            END,
            grandparent_id = excluded.grandparent_id,
            external_ids = excluded.external_ids,
            promoted = excluded.promoted,
            collection_kind = excluded.collection_kind,
            collection_media_kind = excluded.collection_media_kind,
            collection_max_items = excluded.collection_max_items,
            collection_smart_filter = excluded.collection_smart_filter,
            country = excluded.country,
            updated_at = excluded.updated_at,
            certification = excluded.certification,
            certification_age = excluded.certification_age,
            parent_idx = excluded.parent_idx,
            live_start = excluded.live_start,
            live_end = excluded.live_end,
            tvg_id = excluded.tvg_id,
            channel_number = excluded.channel_number,
            enabled = excluded.enabled,
            sort_order = excluded.sort_order,
            custom_name = excluded.custom_name,
            status = excluded.status,
            refreshed_at = COALESCE(excluded.refreshed_at, media.refreshed_at),
            program_kind = excluded.program_kind
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
        .bind(sqlx::types::Json(&self.probe_data))
        .bind(self.promoted)
        .bind(&self.collection_kind)
        .bind(&self.collection_media_kind)
        .bind(self.collection_max_items)
        .bind(sqlx::types::Json(&self.external_ids))
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

        let mut tx = db.begin().await?;
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await?;
        const BATCH_SIZE: usize = 500;

        for chunk in items.chunks(BATCH_SIZE) {
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, description, trailers, stream_info, probe_data, promoted, collection_kind, collection_media_kind,
                external_ids, created_at, updated_at, certification, certification_age, parent_idx,
                live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, grandparent_id, country, program_kind
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
                    .push_bind(sqlx::types::Json(&item.probe_data))
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(sqlx::types::Json(&item.external_ids))
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
                    .push_bind(item.country.as_deref().map(normalize_country_alpha2))
                    .push_bind(&item.program_kind);
            });

            query_builder.push(" ON CONFLICT DO NOTHING");

            query_builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert(db: &sqlx::SqlitePool, items: &[Self]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        const BATCH_SIZE: usize = 500;

        for chunk in items.chunks(BATCH_SIZE) {
            let mut tx = db.begin().await?;
            sqlx::query("PRAGMA defer_foreign_keys = ON")
                .execute(&mut *tx)
                .await?;
            let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO media (
                id, title, kind, parent_id, idx, released_at, runtime,
                rating_critic, rating_audience, description, trailers, stream_info, probe_data, promoted, collection_kind, collection_media_kind,
                external_ids, created_at, updated_at, certification, certification_age, parent_idx,
                live_start, live_end, tvg_id, channel_number, enabled, sort_order, custom_name, digital_released_at, status, refreshed_at, grandparent_id, country, program_kind
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
                    .push_bind(sqlx::types::Json(&item.probe_data))
                    .push_bind(&item.promoted)
                    .push_bind(&item.collection_kind)
                    .push_bind(&item.collection_media_kind)
                    .push_bind(sqlx::types::Json(&item.external_ids))
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
                    .push_bind(&item.refreshed_at)
                    .push_bind(&item.grandparent_id)
                    .push_bind(item.country.as_deref().map(normalize_country_alpha2))
                    .push_bind(&item.program_kind);
            });

            query_builder.push(
                " ON CONFLICT DO UPDATE SET
                title = excluded.title,
                idx = excluded.idx,
                released_at = excluded.released_at,
                digital_released_at = excluded.digital_released_at,
                runtime = excluded.runtime,
                rating_critic = excluded.rating_critic,
                rating_audience = excluded.rating_audience,
                description = excluded.description,
                trailers = excluded.trailers,
                stream_info = excluded.stream_info,
                external_ids = excluded.external_ids,
                probe_data = CASE
                WHEN excluded.stream_info IS NOT media.stream_info THEN NULL
                ELSE COALESCE(excluded.probe_data, media.probe_data)
            END,
                grandparent_id = excluded.grandparent_id,
                updated_at = excluded.updated_at,
                promoted = excluded.promoted,
                certification = excluded.certification,
                certification_age = excluded.certification_age,
                parent_id = excluded.parent_id,
                parent_idx = excluded.parent_idx,
                live_start = excluded.live_start,
                live_end = excluded.live_end,
                tvg_id = excluded.tvg_id,
                channel_number = excluded.channel_number,
                status = excluded.status,
                country = excluded.country,
                refreshed_at = COALESCE(excluded.refreshed_at, media.refreshed_at),
                -- preserve user overrides: only update name/enabled/sort_order if not set by user
                title = CASE WHEN custom_name IS NOT NULL THEN media.title ELSE excluded.title END,
                enabled = CASE WHEN media.id IS NOT NULL THEN media.enabled ELSE excluded.enabled END,
                sort_order = CASE WHEN media.id IS NOT NULL THEN media.sort_order ELSE excluded.sort_order END,
                custom_name = media.custom_name,
                program_kind = excluded.program_kind",
            );

            query_builder.build().execute(&mut *tx).await?;

            let chunk_images: Vec<(Uuid, &MediaImage)> = chunk
                .iter()
                .flat_map(|m| m.images.iter().map(move |img| (m.id, img)))
                .collect();
            for img_chunk in chunk_images.chunks(500) {
                let mut qb = sqlx::QueryBuilder::new(
                    "INSERT OR IGNORE INTO media_images \
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
                qb.build().execute(&mut *tx).await?;
            }

            tx.commit().await?;
        }

        Ok(())
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
            qb.push(" WHERE g.kind = 'genre' AND m.kind IN (");
            let mut sep = qb.separated(", ");
            for k in related_kinds {
                sep.push_bind(k);
            }
            qb.push(")");
        } else {
            qb.push(" WHERE g.kind = 'genre'");
        }

        qb.push(" ORDER BY g.title ASC");

        Ok(qb.build_query_as::<Self>().fetch_all(db).await?)
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
        let rows = qb.build().fetch_all(db).await?;
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
        let use_recursive = filter.recursive && filter.parent_id.is_some();

        let mut count_qb;
        let mut records_qb;

        if use_recursive {
            let parent_id = filter.parent_id.as_ref().unwrap();

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
            records_qb.push(
                " UNION ALL SELECT m.id FROM media m INNER JOIN subtree s ON m.parent_id = s.id\
                ) SELECT * FROM media WHERE id IN (SELECT id FROM subtree) AND 1=1",
            );
        } else {
            count_qb = sqlx::QueryBuilder::new(
                "SELECT COUNT(*) as count FROM media WHERE 1=1",
            );
            records_qb = sqlx::QueryBuilder::new("SELECT * FROM media WHERE 1=1");
        }

        // Pre-fetch in-progress media IDs — JOIN media so kind and date filters are applied
        // here rather than in the main query. The main query then contains only
        // `WHERE media.id IN (ids)` which forces SQLite to use individual PK lookups
        // (O(n_ids)) instead of scanning the entire kind-filtered media table (O(total_media)).
        let resumable_ids: Option<Vec<uuid::Uuid>> = if let Some(usf) =
            &filter.user_state
        {
            if usf.resumable == Some(true) {
                let ids: Vec<uuid::Uuid> = if let Some(user_id) = &usf.user_id {
                    let mut pre_qb = sqlx::QueryBuilder::new(
                        "SELECT ums.media_id FROM user_media_state ums \
                         JOIN media m ON m.id = ums.media_id \
                         WHERE ums.user_id = ",
                    );
                    pre_qb.push_bind(user_id);
                    pre_qb
                        .push(" AND ums.playback_position > 0 AND ums.play_count = 0");
                    if let Some(kinds) = &filter.kind {
                        if !kinds.is_empty() {
                            pre_qb.push(" AND m.kind IN (");
                            let mut sep = pre_qb.separated(", ");
                            for k in kinds {
                                sep.push_bind(k);
                            }
                            pre_qb.push(")");
                        }
                    }
                    if let Some(threshold) = &filter.digital_released_before {
                        pre_qb
                            .push(" AND COALESCE(m.digital_released_at, m.released_at) <= ")
                            .push_bind(threshold);
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

        for qb in [&mut count_qb, &mut records_qb] {
            if !use_recursive {
                if let Some(parent_id) = &filter.parent_id {
                    qb.push(" AND parent_id = ").push_bind(parent_id);
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
            if let Some(grandparent_id) = &filter.grandparent_id {
                qb.push(" AND grandparent_id = ").push_bind(grandparent_id);
            }
            if let Some(promoted) = &filter.promoted {
                qb.push(" AND promoted = ").push_bind(promoted);
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
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.favorite = ")
                        .push_bind(favorite)
                        .push(")");
                }

                // played=true — EXISTS with play_count > 0
                if user_state_filter.played == Some(true) {
                    qb.push(" AND EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_id = media.id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
                    }
                    qb.push(" AND ums.play_count > 0)");
                }

                // played=false (unplayed) — NOT EXISTS with play_count > 0
                if user_state_filter.played == Some(false) {
                    qb.push(" AND NOT EXISTS (SELECT 1 FROM user_media_state ums WHERE ums.media_id = media.id");
                    if let Some(user_id) = &user_state_filter.user_id {
                        qb.push(" AND ums.user_id = ").push_bind(user_id);
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
                qb.push(" AND title LIKE ").push_bind(format!("{}%", s));
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
                qb.push(" AND title LIKE ").push_bind(format!("%{}%", s));
            }

            if let Some(idx) = &filter.index_number {
                qb.push(" AND idx = ").push_bind(idx);
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
                qb.push(" AND enabled = ").push_bind(*enabled);
            }

            if let Some(c) = &filter.country_filter {
                qb.push(" AND country = ").push_bind(c.to_uppercase());
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
                qb.push(" AND live_end >= ").push_bind(min_end);
            }

            if let Some(max_start) = &filter.max_start_date {
                qb.push(" AND live_start <= ").push_bind(max_start);
            }

            if let Some(kinds) = &filter.program_kinds {
                if !kinds.is_empty() {
                    qb.push_in("program_kind", kinds);
                }
            }

            if let Some(threshold) = &filter.digital_released_before {
                if resumable_ids.is_none() {
                    qb.push(" AND COALESCE(digital_released_at, released_at) <= ")
                        .push_bind(threshold);
                }
            }

            if !filter.filter_rules.is_empty() {
                apply_filter_rules(qb, &filter.filter_rules, &filter.filter_match);
            }
        }
        // Apply ORDER BY driven by the sort_by field, with per-kind fallbacks.
        let is_channel_query = filter
            .kind
            .as_ref()
            .map(|k| k.iter().all(|k| matches!(k, MediaKind::TvChannel)))
            .unwrap_or(false);

        if !filter.sort_by.is_empty() {
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
                        api::ItemSortBy::Random => "RANDOM()".to_string(),
                        api::ItemSortBy::ChannelOrder => {
                            format!("(sort_order IS NULL), COALESCE(sort_order, channel_number, 999999) {dir}, title COLLATE NOCASE")
                        }
                        // Default fallback
                        _ => format!("title COLLATE NOCASE {}", dir),
                    };
                    col
                })
                .collect();
            records_qb.push(" ORDER BY ");
            records_qb.push(order_clauses.join(", "));
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
            records_qb.push(" LIMIT ").push_bind(limit);
        } else if filter.offset.is_some() {
            records_qb.push(" LIMIT -1");
        }
        if let Some(offset) = &filter.offset {
            records_qb.push(" OFFSET ").push_bind(offset);
        }

        let (count, records_result) = tokio::join!(
            async {
                let query = count_qb.build();
                let row = query.fetch_one(db).await;
                row.map(|r| r.get::<i64, _>(0) as usize)
            },
            async {
                let query = records_qb.build_query_as::<Media>();
                query.fetch_all(db).await
            }
        );

        let mut records = records_result?;

        // Batch-load tags for all fetched records
        if !records.is_empty() {
            let ids: Vec<Uuid> = records.iter().map(|m| m.id).collect();
            let mut tags_qb = sqlx::QueryBuilder::new(
                "SELECT media_id, tag FROM media_tags WHERE media_id IN (",
            );
            let mut sep = tags_qb.separated(", ");
            for id in &ids {
                sep.push_bind(id);
            }
            tags_qb.push(") ORDER BY tag");
            let tag_rows = tags_qb.build().fetch_all(db).await?;
            let mut tags_map: HashMap<Uuid, Vec<String>> = HashMap::new();
            for row in tag_rows {
                let media_id: Uuid = row.get(0);
                let tag: String = row.get(1);
                tags_map.entry(media_id).or_default().push(tag);
            }
            for media in &mut records {
                if let Some(tags) = tags_map.remove(&media.id) {
                    media.tags = tags;
                }
            }

            // Batch-load images
            let mut images_map = MediaImage::get_for_media_ids(db, &ids)
                .await
                .unwrap_or_default();
            for media in &mut records {
                media.images = images_map.remove(&media.id).unwrap_or_default();
            }
        }

        // Batch-load genre and person relations for Movie/Episode/Series/Season records
        let rel_ids: Vec<Uuid> = records
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
            .collect();
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
            match g_qb.build().fetch_all(db).await {
                Ok(rows) => {
                    let mut rels_map: HashMap<Uuid, Vec<(MediaRelation, Media)>> =
                        HashMap::new();
                    for row in rows {
                        let kind_str: String = row.get(8);
                        let Ok(kind) = MediaKind::try_from(kind_str) else {
                            continue;
                        };
                        if !matches!(kind, MediaKind::Genre | MediaKind::Person) {
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
                    // Batch-load images for the related nodes (persons, genres)
                    let related_ids: Vec<Uuid> = rels_map
                        .values()
                        .flat_map(|v| v.iter().map(|(_, m)| m.id))
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
                    tracing::warn!("failed to batch-load relations: {e}");
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
                match cc_qb.build().fetch_all(db).await {
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
                        tracing::warn!("failed to load child counts: {e}");
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
                match pl_qb.build().fetch_all(db).await {
                    Ok(rows) => {
                        let mut cc_map: HashMap<Uuid, i64> = HashMap::new();
                        for row in rows {
                            let pid: Uuid = row.get(0);
                            let cnt: i64 = row.get(1);
                            cc_map.insert(pid, cnt);
                        }
                        for media in &mut records {
                            if media.kind == MediaKind::Playlist {
                                media.child_count =
                                    Some(*cc_map.get(&media.id).unwrap_or(&0));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("failed to load playlist child counts: {e}");
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
                if let Ok(rows) = ep_qb.build().fetch_all(db).await {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Series {
                            media.recursive_item_count = map.get(&media.id).copied();
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
                if let Ok(rows) = movie_qb.build().fetch_all(db).await {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Person {
                            media.movie_count = map.get(&media.id).copied();
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
                if let Ok(rows) = series_qb.build().fetch_all(db).await {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Person {
                            media.series_count = map.get(&media.id).copied();
                        }
                    }
                }

                // child_count = movie_count + series_count
                for media in &mut records {
                    if media.kind == MediaKind::Person {
                        media.child_count = Some(
                            media.movie_count.unwrap_or(0)
                                + media.series_count.unwrap_or(0),
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
                if let Ok(rows) = alb_qb.build().fetch_all(db).await {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Artist {
                            media.album_count = map.get(&media.id).copied();
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
                if let Ok(rows) = song_qb.build().fetch_all(db).await {
                    let mut map: HashMap<Uuid, i64> = HashMap::new();
                    for row in rows {
                        map.insert(row.get(0), row.get(1));
                    }
                    for media in &mut records {
                        if media.kind == MediaKind::Artist {
                            media.song_count = map.get(&media.id).copied();
                        }
                    }
                }
            }
        }

        Self::enrich_parents(db, &mut records).await;

        if filter.include_user_state {
            let uid = filter
                .user_id
                .or_else(|| filter.user_state.as_ref().and_then(|s| s.user_id));
            if let Some(user_id) = uid {
                let media_ids: Vec<Uuid> = records.iter().map(|m| m.id).collect();

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
                    qb.push(" AND ums.play_count > 0) GROUP BY e.grandparent_id");

                    match qb.build().fetch_all(db).await {
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
                            tracing::warn!("failed to load unplayed counts: {e}");
                        }
                    }
                }
            }
        }

        Ok(FilterResult {
            records,
            total_count: if filter.total_count { count? } else { 0 },
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
            OR (kind = 'series' AND (status IS NULL OR status != 'ended'))
            OR digital_released_at IS NULL
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
    ) -> Result<FilterResult<Media>> {
        let user_policy = user.and_then(|u| u.policy.as_ref()).map(|p| &p.0);
        // Map media_types (Video, Book, ...) to MediaKind constraints
        let media_type_kinds: Option<Vec<MediaKind>> =
            filter.media_types.as_ref().map(|types| {
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
            let ikt_kinds: Vec<MediaKind> = include_item_types
                .iter()
                .filter_map(|t| MediaKind::try_from(t.clone()).ok())
                .collect();
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
                    let rows = qb.build().fetch_all(db).await?;
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
                    let rows = qb.build().fetch_all(db).await?;
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
            let from_param: Option<Vec<Uuid>> = filter.genre_ids.as_ref().map(|ids| {
                ids.iter()
                    .flat_map(|s| s.split(','))
                    .filter_map(|s| s.trim().parse::<Uuid>().ok())
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
            let from_param: Option<Vec<Uuid>> = filter.studio_ids.as_ref().map(|ids| {
                ids.iter()
                    .flat_map(|s| s.split(','))
                    .filter_map(|s| s.trim().parse::<Uuid>().ok())
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

        let person_ids: Option<Vec<Uuid>> = filter.person_ids.as_ref().map(|ids| {
            ids.iter()
                .flat_map(|s| s.split(','))
                .filter_map(|s| s.trim().parse::<Uuid>().ok())
                .collect()
        });

        // Build user-state filter from is_favorite + filters[] items
        let item_filters = filter.filters.as_deref().unwrap_or(&[]);
        let is_played = item_filters.contains(&api::ItemFilter::IsPlayed);
        let is_unplayed = item_filters.contains(&api::ItemFilter::IsUnplayed);
        let is_resumable = item_filters.contains(&api::ItemFilter::IsResumable);
        let favorite = filter.is_favorite.or_else(|| {
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

        let release_date_applies = !kinds.is_empty()
            && kinds.iter().any(|k| {
                matches!(
                    k,
                    MediaKind::Movie
                        | MediaKind::Series
                        | MediaKind::Season
                        | MediaKind::Episode
                )
            });
        let digital_released_before = if release_date_applies
            && server_config.map(|c| c.filter_by_digital_release_date) != Some(false)
        {
            let buffer = server_config
                .map(|c| c.digital_release_buffer_days)
                .unwrap_or(0);
            Some(Utc::now().naive_utc() + Duration::days(buffer))
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

        let user_policy_filter = user_policy.and_then(|p| p.filter_rules.as_ref());

        let mut result = Self::get_by_filter(
            db,
            &MediaFilter {
                kind: Some(kinds),
                enabled: has_tv_channel.then_some(true),
                promoted: filter.promoted,
                limit: filter.limit.clone(),
                id: filter.ids.clone(),
                // album_ids maps directly to parent_id (tracks are children of albums)
                parent_id: filter.parent_id.clone().or_else(|| {
                    filter.album_ids.as_ref().and_then(|v| v.first().cloned())
                }),
                offset: filter.start_index.clone(),
                recursive: filter.recursive,
                include_user_state: filter.enable_user_data.is_none(),
                user_id: filter.user_id,
                include_child_count: has_playlist
                    || filter
                        .fields
                        .as_deref()
                        .map(|f| f.contains(&api::ItemFields::ChildCount))
                        .unwrap_or(false),
                total_count,
                user_state,
                genre_ids,
                studio_ids,
                person_ids,
                years: filter.years.clone(),
                official_ratings: filter.official_ratings.clone(),
                max_parental_rating: user_policy.and_then(|p| p.max_parental_rating),
                name_starts_with: filter.name_starts_with.clone(),
                name_starts_with_or_greater: filter.name_starts_with_or_greater.clone(),
                name_less_than: filter.name_less_than.clone(),
                title_contains: filter.search_term.clone(),
                index_number: filter.index_number,
                has_trailer: filter.has_trailer,
                tags: filter.tags.clone(),
                blocked_tags: user_policy
                    .map(|p| p.blocked_tags.clone())
                    .filter(|v| !v.is_empty()),
                allowed_tags: user_policy
                    .map(|p| p.allowed_tags.clone())
                    .filter(|v| !v.is_empty()),
                digital_released_before,
                sort_by: filter.sort_by.clone().unwrap_or_default(),
                sort_order: filter.sort_order.clone().unwrap_or_default(),
                filter_rules: {
                    let mut rules =
                        smart_filter.map(|sf| sf.rules.clone()).unwrap_or_default();
                    // Content filter rules must not apply to container queries — only
                    // to content (movies, episodes, etc.). See CLAUDE.md.
                    if !targeting_containers {
                        if let Some(pf) = user_policy_filter {
                            rules.extend(pf.rules.clone());
                        }
                    }
                    rules
                },
                filter_match: smart_filter
                    .map(|sf| sf.match_mode.clone())
                    .unwrap_or_default(),
                artist_ids: filter
                    .artist_ids
                    .clone()
                    .or_else(|| filter.contributing_artist_ids.clone())
                    .or_else(|| filter.album_artist_ids.clone()),
                grandparent_id: filter.series_id,
                ..Default::default()
            },
        )
        .await?;

        // Hide containers that contain zero items visible to the user after applying
        // their content filter rules.
        if targeting_containers && !result.records.is_empty() {
            if let Some(pf) = user_policy_filter {
                if !pf.rules.is_empty() {
                    let container_ids: Vec<uuid::Uuid> =
                        result.records.iter().map(|m| m.id).collect();
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
                    apply_filter_rules(&mut qb, &pf.rules, &pf.match_mode);
                    qb.push(" GROUP BY parent_id");

                    if let Ok(rows) = qb.build().fetch_all(db).await {
                        let counts: HashMap<uuid::Uuid, i64> = rows
                            .into_iter()
                            .map(|r| (r.get::<uuid::Uuid, _>(0), r.get::<i64, _>(1)))
                            .collect();
                        result
                            .records
                            .retain(|m| counts.get(&m.id).copied().unwrap_or(0) > 0);
                        result.total_count = result.records.len();
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
            type_: self.kind.clone().into(),
            parent_id: self.parent_id,
            index_number: self.idx,
            name: Some(match self.kind {
                MediaKind::Episode => format!("Episode {}", self.idx.unwrap_or(0)),
                MediaKind::Season => format!("Season {}", self.idx.unwrap_or(0)),
                _ => self.title.clone(),
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

    pub async fn mark_played(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = 1;
        state.played_at = Some(Local::now().naive_local());
        state.save(db).await?;
        Ok(state)
    }

    pub async fn mark_unplayed(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.play_count = 0;
        state.played_at = None;
        state.playback_position = 0;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn mark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = true;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn unmark_favorite(
        &self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<super::UserMediaState> {
        let mut state = super::UserMediaState::get_or_new(db, user, self).await?;
        state.favorite = false;
        state.save(db).await?;
        Ok(state)
    }

    pub async fn streams(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.sources.is_none() {
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

            sources.sort_by(|a, b| a.idx.cmp(&b.idx));

            // Exclude Sources that predate the last refresh — they belong to a
            // previous fetch and may have expired URLs. They stay in the DB so
            // an ongoing playback session can still reach them by direct ID.
            if let Some(refreshed) = self.streams_refreshed_at {
                sources.retain(|s| s.updated_at >= refreshed);
            }

            self.sources = Some(sources);
        };
        Ok(self.sources.as_deref().unwrap_or_default().to_vec())
    }

    pub async fn seasons(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Series {
            return Ok(vec![]);
        }

        if self.seasons.is_none() {
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

        Ok(self.seasons.as_deref().unwrap_or_default().to_vec())
    }

    pub async fn episodes(&mut self, db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
        if self.kind != MediaKind::Season {
            return Ok(vec![]);
        }

        if self.episodes.is_none() {
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

        Ok(self.episodes.as_deref().unwrap_or_default().to_vec())
    }

    pub async fn user_state(
        &mut self,
        db: &SqlitePool,
        user: &super::User,
    ) -> Result<Option<super::UserMediaState>> {
        if self.user_state.is_none() {
            let state = super::UserMediaState::get_or_new(db, user, self).await?;

            self.user_state = Some(state);
        }

        Ok(self.user_state.clone())
    }

    pub async fn load_relations(&mut self, db: &SqlitePool) -> Result<()> {
        if self.relations.is_some() {
            return Ok(());
        }

        let rels = MediaRelation::get_by_media_id(db, &self.id).await?;
        if rels.is_empty() {
            self.relations = Some(vec![]);
            return Ok(());
        }

        let media_ids: Vec<Uuid> = rels.iter().map(|r| r.right_media_id).collect();
        let related = Self::get_by_filter(
            db,
            &MediaFilter {
                id: Some(media_ids),
                ..Default::default()
            },
        )
        .await?
        .records;

        let map: std::collections::HashMap<Uuid, Media> =
            related.into_iter().map(|m| (m.id, m)).collect();

        let pairs = rels
            .into_iter()
            .filter_map(|rel| map.get(&rel.right_media_id).map(|m| (rel, m.clone())))
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
                file_hint: source.filename.clone(),
                file_idx: source.file_idx.map(|i| i as usize),
                trackers: source
                    .sources
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|src| src.strip_prefix("tracker:"))
                    .map(String::from)
                    .collect(),
            }
        } else if let Some(url) =
            source.url.clone().or_else(|| source.external_url.clone())
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
            filename: source.filename.clone(),
            name: source.name.clone(),
            description: source.description.clone(),
            seeders: source.seeders,
            size: source.size,
            duration: source.duration,
            subtitles: source.subtitles.clone(),
            probe_data: None,
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

        let mut media_kind =
            MediaKind::try_from(meta.media_type.clone()).unwrap_or(MediaKind::Movie);
        if media_kind == MediaKind::Movie
            && meta.videos.as_ref().map_or(false, |v| !v.is_empty())
        {
            media_kind = MediaKind::Series;
        }

        let digital_released_at = meta
            .app_extras
            .as_ref()
            .and_then(|e| e.release_dates.as_ref())
            .map(|rd| {
                {
                    rd.results
                        .iter()
                        .flat_map(|country| country.release_dates.iter())
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
                    meta.released.map(|x| x.naive_utc())
                } else {
                    None
                }
            });

        let status = meta.status.as_ref().map(|s| match s {
            sdks::stremio::Status::Continuing
            | sdks::stremio::Status::ReturningSeries
            | sdks::stremio::Status::InProduction
            | sdks::stremio::Status::Running => MediaStatus::Continuing,
            sdks::stremio::Status::Ended | sdks::stremio::Status::Canceled => {
                MediaStatus::Ended
            }
            sdks::stremio::Status::Upcoming | sdks::stremio::Status::Planned => {
                MediaStatus::Unreleased
            }
            sdks::stremio::Status::Unknown => MediaStatus::Continuing,
        });

        let media = Media {
            title: meta.get_name().unwrap_or_default(),
            kind: media_kind.clone(),
            released_at: meta.released.map(|x| x.naive_utc()),
            digital_released_at,
            runtime: meta.runtime.map(|d| d.num_seconds()),
            // rating_critic: meta.rating_critic,
            rating_audience: meta.imdb_rating,
            description: meta.description,
            certification: meta.certification.clone(),
            certification_age: {
                let country = meta
                    .country
                    .as_ref()
                    .and_then(|v| v.first())
                    .map(|c| normalize_country_alpha2(c));
                crate::localization::ratings::resolve_rating_age(
                    meta.certification.as_deref(),
                    country.as_deref(),
                )
            },
            country: meta
                .country
                .and_then(|v| v.into_iter().next())
                .map(|c| normalize_country_alpha2(&c)),
            external_ids: {
                let mut ids = ExternalIds::from_stremio_id(&meta.id);
                if let Some(ref imdb) = meta.imdb_id {
                    ids.imdb = Some(imdb.clone());
                }
                ids
            },
            status,
            trailers: meta.trailers.map(|trailers| {
                trailers
                    .into_iter()
                    .map(|t| t.source)
                    .collect::<Vec<String>>()
            }),
            id: meta
                .imdb_id
                .as_ref()
                .map(|mid| {
                    Uuid::from(&super::MediaIdRaw {
                        kind: media_kind.clone(),
                        external_ids: ExternalIds {
                            imdb: Some(mid.to_string()),
                            ..Default::default()
                        },
                        season: None,
                        episode: None,
                    })
                })
                .unwrap_or_else(uuid::Uuid::new_v4),
            ..Default::default()
        };

        let mut media = media;
        if let Some(url) = meta.poster.or(meta.thumbnail) {
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
    let imdb_id = meta.imdb_id.clone().context("imdb_id is missing")?;

    let mut media: Media = meta.clone().try_into()?;
    media.id = Uuid::from(&super::MediaIdRaw {
        kind: media.kind.clone(),
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
                    .filter_map(|ep| ep.season.map(|s| (s, ep.clone())))
                    .fold(
                        std::collections::BTreeMap::new(),
                        |mut acc, (season, ep)| {
                            acc.entry(season).or_default().push(ep);
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
                    let mut episode: Media = ep.clone().try_into()?;
                    let ep_idx = ep.episode.unwrap_or(0);
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
                        ..Default::default()
                    };
                    episode.grandparent_id = Some(media.id);
                    episode.parent_id = Some(season.id);
                    episode.parent_idx = Some(season_idx);
                    episode.released_at = ep.released.map(|x| x.naive_utc());
                    episode.digital_released_at = ep.released.map(|x| x.naive_utc());

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
    rules: &[remux_sdks::remux::FilterRule],
    match_mode: &remux_sdks::remux::FilterMatchMode,
) {
    use remux_sdks::remux::FilterMatchMode;

    if rules.is_empty() {
        return;
    }

    let is_any = *match_mode == FilterMatchMode::Any;
    if is_any {
        qb.push(" AND (");
    }

    let mut first = true;
    for rule in rules {
        if let Some((sql, negated)) = filter_rule_to_sql(rule) {
            if is_any {
                if !first {
                    qb.push(" OR ");
                }
            } else {
                qb.push(" AND ");
            }
            first = false;
            if negated {
                qb.push("NOT (");
            }
            qb.push(sql);
            if negated {
                qb.push(")");
            }
        }
    }

    if is_any && !first {
        qb.push(")");
    }
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
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
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
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
                    format!("lower(country) = lower('{v}')")
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!("lower(country) IN ({list})")
                }
            };
            Some((sql, negated))
        }
        R::Tag { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
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
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
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
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
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
        R::Collection { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
                    format!(
                        "EXISTS (SELECT 1 FROM media_catalog_items mci \
                         WHERE mci.media_id = media.id \
                         AND lower(mci.addon_id || ':' || mci.catalog_id) = lower('{v}'))"
                    )
                }
                SetOp::In | SetOp::NotIn => {
                    let list = in_list(values)?;
                    format!(
                        "EXISTS (SELECT 1 FROM media_catalog_items mci \
                         WHERE mci.media_id = media.id \
                         AND lower(mci.addon_id || ':' || mci.catalog_id) IN ({list}))"
                    )
                }
            };
            Some((sql, negated))
        }
        R::Person { op, values } => {
            let negated = matches!(op, SetOp::IsNot | SetOp::NotIn);
            let sql = match op {
                SetOp::Is | SetOp::IsNot => {
                    let v = esc(values.first().map(|s| s.as_str()).unwrap_or(""));
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
            .flat_map(|s| s.split(',').map(|n| n.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        for (i, name) in names.into_iter().enumerate() {
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
        ep.directors.as_ref(),
        RelationRole::Director,
    );
    add_names(&mut relations, ep.writers.as_ref(), RelationRole::Writer);
    relations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_tmdb_in_directory() {
        let ids = ExternalIds::from_path(
            "Movies/The Matrix (1999) [tmdbid-603]/The Matrix.mkv",
        );
        assert_eq!(ids.tmdb, Some(603));
        assert!(ids.imdb.is_none());
        assert!(ids.tvdb.is_none());
    }

    #[test]
    fn from_path_tvdb_in_directory() {
        let ids = ExternalIds::from_path(
            "TV/Breaking Bad [tvdbid-81189]/Season 1/S01E01.mkv",
        );
        assert_eq!(ids.tvdb, Some(81189));
        assert!(ids.tmdb.is_none());
    }

    #[test]
    fn from_path_imdb_in_filename() {
        let ids = ExternalIds::from_path("[imdbid-tt0133093] The Matrix 1999.mkv");
        assert_eq!(ids.imdb.as_deref(), Some("tt0133093"));
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
}
