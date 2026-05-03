use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::Stream;
use futures::stream::{self, StreamExt};
use remux_sdks::stremio::MediaType;
use serde::Deserialize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, CatalogInfo, MusicSearchResult, ResourceType,
};
use crate::db::MetaResult;
use crate::{AppContext, common, db};

const BASE: &str = "https://api.deezer.com";

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .build()
        .expect("failed to build HTTP client")
}

fn parse_release_date(s: &str) -> Option<NaiveDateTime> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
}

// ---------------------------------------------------------------------------
// Shared Deezer response wrapper
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum DeezerResponse<T> {
    Ok(T),
    Err { error: serde_json::Value },
}

// ---------------------------------------------------------------------------
// Meta types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MetaTrackDetail {
    id: u64,
    title: String,
    duration: u64,
    release_date: Option<String>,
    artist: MetaTrackArtist,
    album: MetaTrackAlbum,
}

#[derive(Deserialize)]
struct MetaTrackArtist {
    name: String,
}

#[derive(Deserialize)]
struct MetaTrackAlbum {
    cover_xl: Option<String>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct MetaAlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<MetaGenreList>,
    artist: Option<MetaAlbumArtist>,
    #[serde(default)]
    nb_tracks: u32,
}

#[derive(Deserialize)]
struct MetaGenreList {
    data: Vec<MetaGenre>,
}

#[derive(Deserialize)]
struct MetaGenre {
    name: String,
}

#[derive(Deserialize)]
struct MetaAlbumArtist {
    name: String,
}

#[derive(Deserialize)]
struct MetaArtistDetail {
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize, Default)]
struct MetaArtistAlbumList {
    data: Vec<MetaArtistAlbumSummary>,
}

#[derive(Deserialize)]
struct MetaArtistAlbumSummary {
    id: u64,
}

#[derive(Deserialize, Default)]
struct MetaFullAlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<MetaGenreList>,
    artist: Option<MetaArtistRef>,
    #[serde(default)]
    tracks: MetaAlbumTrackList,
}

#[derive(Deserialize)]
struct MetaArtistRef {
    name: String,
}

#[derive(Deserialize, Default)]
struct MetaAlbumTrackList {
    data: Vec<MetaAlbumTrackSummary>,
}

#[derive(Deserialize)]
struct MetaAlbumTrackSummary {
    id: u64,
    title: String,
    duration: Option<u64>,
    track_position: Option<i64>,
    disk_number: Option<i64>,
    artist: MetaTrackArtistRef,
}

#[derive(Deserialize)]
struct MetaTrackArtistRef {
    name: String,
}

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchDeezerArtist {
    id: u64,
    name: String,
    picture_medium: Option<String>,
}

#[derive(Deserialize)]
struct SearchArtistList {
    data: Vec<SearchArtistItem>,
}

#[derive(Deserialize)]
struct SearchArtistItem {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize)]
struct SearchAlbumRef {
    id: u64,
    title: String,
    cover_medium: Option<String>,
}

#[derive(Deserialize)]
struct SearchArtistDetail {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize, Default)]
struct SearchAlbumList {
    data: Vec<SearchAlbumSummary>,
}

#[derive(Deserialize)]
struct SearchAlbumSummary {
    id: u64,
}

#[derive(Deserialize)]
struct SearchAlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<SearchGenreList>,
    artist: Option<SearchArtistRef>,
    #[serde(default)]
    tracks: SearchTrackList,
}

#[derive(Deserialize, Default)]
struct SearchTrackList {
    data: Vec<SearchTrackSummary>,
}

#[derive(Deserialize)]
struct SearchGenreList {
    data: Vec<SearchGenre>,
}

#[derive(Deserialize)]
struct SearchGenre {
    name: String,
}

#[derive(Deserialize)]
struct SearchArtistRef {
    name: String,
}

#[derive(Deserialize)]
struct SearchTrackSummary {
    id: u64,
    title: String,
    duration: Option<u64>,
    track_position: Option<i64>,
    disk_number: Option<i64>,
    artist: SearchDeezerArtist,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DeezerDiscographyResponse<T> {
    Ok(T),
    Err { error: serde_json::Value },
}

#[derive(Deserialize)]
struct TrackSearchResult {
    data: Vec<SearchTrack>,
}

#[derive(Deserialize)]
struct SearchTrack {
    id: u64,
    title: String,
    duration: Option<u64>,
    artist: SearchDeezerArtist,
    album: SearchAlbumRef,
}

#[derive(Deserialize)]
struct AlbumSearchResult {
    data: Vec<SearchAlbum>,
}

#[derive(Deserialize)]
struct SearchAlbum {
    id: u64,
    title: String,
    cover_medium: Option<String>,
    artist: SearchDeezerArtist,
}

// ---------------------------------------------------------------------------
// Playlist types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PlaylistResponse {
    tracks: PlaylistTracks,
}

#[derive(Deserialize)]
struct PlaylistTracks {
    data: Vec<PlaylistTrack>,
}

#[derive(Deserialize)]
struct PlaylistTrack {
    id: u64,
    title: String,
    duration: u64,
    artist: PlaylistTrackArtist,
    album: PlaylistTrackAlbum,
}

#[derive(Deserialize)]
struct PlaylistTrackArtist {
    name: String,
}

#[derive(Deserialize)]
struct PlaylistTrackAlbum {
    cover_xl: Option<String>,
    release_date: Option<String>,
}

// ---------------------------------------------------------------------------
// AddonKind registration
// ---------------------------------------------------------------------------

pub struct DeezerPreset;

impl AddonPreset for DeezerPreset {
    fn id(&self) -> &'static str {
        "deezer"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "deezer".to_string(),
            display_name: "Deezer".to_string(),
            description:
                "Public Deezer API — music metadata, search, and your own playlists \
                 surfaced as catalogs."
                    .to_string(),
            icon: None,
            supported_resources: vec![
                ResourceType::Catalog,
                ResourceType::Meta,
                ResourceType::Search,
            ],
            supported_types: vec![
                MediaType::Track,
                MediaType::Album,
                MediaType::Artist,
            ],
            options: vec![AddonOption {
                id: "playlists".to_string(),
                name: "Playlist IDs".to_string(),
                description: Some(
                    "Deezer playlist IDs to expose as catalogs. One per row."
                        .to_string(),
                ),
                required: false,
                default: None,
                kind: AddonOptionType::StringList,
            }],
        }
    }

    fn from_cfg(&self, cfg: &serde_json::Value) -> Result<Arc<dyn AddonKind>> {
        let playlists = cfg
            .get("playlists")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| extract_playlist_id(&s))
            .collect();
        Ok(Arc::new(DeezerAddon {
            playlists,
            client: build_client(),
        }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(DeezerPreset))
}

// ---------------------------------------------------------------------------
// Addon struct
// ---------------------------------------------------------------------------

pub struct DeezerAddon {
    playlists: Vec<String>,
    client: reqwest::Client,
}

impl DeezerAddon {
    fn playlists(&self) -> &[String] {
        &self.playlists
    }

    // --- Meta ---

    fn meta_can_refresh(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Track | db::MediaKind::Album)
            && media
                .media_id
                .as_deref()
                .is_some_and(|id| id.parse::<u64>().is_ok())
    }

    async fn fetch_track_meta(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<MetaResult>> {
        let url = format!("{}/track/{}", BASE, deezer_id);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(deezer_id, status = %resp.status(), "Deezer track detail HTTP error");
            return Ok(None);
        }
        let t: MetaTrackDetail =
            match resp.json::<DeezerResponse<MetaTrackDetail>>().await? {
                DeezerResponse::Ok(t) => t,
                DeezerResponse::Err { error } => {
                    warn!(deezer_id, %error, "Deezer track detail returned error");
                    return Ok(None);
                }
            };
        let released_at = t
            .release_date
            .as_deref()
            .or(t.album.release_date.as_deref())
            .and_then(parse_release_date);
        tracing::debug!(deezer_id, "Deezer track detail fetched");
        Ok(Some(MetaResult {
            media: db::Media {
                id: base.id,
                title: t.title,
                kind: db::MediaKind::Track,
                media_id: Some(t.id.to_string()),
                poster: t.album.cover_xl,
                runtime: Some(t.duration as i64),
                released_at,
                description: Some(format!("by {}", t.artist.name)),
                ..Default::default()
            },
            relations: vec![],
        }))
    }

    async fn fetch_album_meta(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<MetaResult>> {
        let url = format!("{}/album/{}", BASE, deezer_id);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(deezer_id, status = %resp.status(), "Deezer album detail HTTP error");
            return Ok(None);
        }
        let a: MetaAlbumDetail =
            match resp.json::<DeezerResponse<MetaAlbumDetail>>().await? {
                DeezerResponse::Ok(a) => a,
                DeezerResponse::Err { error } => {
                    warn!(deezer_id, %error, "Deezer album detail returned error");
                    return Ok(None);
                }
            };
        let released_at = a.release_date.as_deref().and_then(parse_release_date);
        let genre_names = a
            .genres
            .as_ref()
            .map(|g| g.data.iter().map(|g| g.name.clone()).collect::<Vec<_>>());
        let mut desc_parts: Vec<String> = vec![];
        if let Some(artist) = &a.artist {
            desc_parts.push(format!("by {}", artist.name));
        }
        if let Some(genres) = &genre_names {
            if !genres.is_empty() {
                desc_parts.push(genres.join(", "));
            }
        }
        if let Some(label) = &a.label {
            desc_parts.push(label.clone());
        }
        tracing::debug!(
            deezer_id,
            nb_tracks = a.nb_tracks,
            "Deezer album detail fetched"
        );
        Ok(Some(MetaResult {
            media: db::Media {
                id: base.id,
                title: a.title,
                kind: db::MediaKind::Album,
                media_id: Some(a.id.to_string()),
                poster: a.cover_xl,
                released_at,
                description: Some(desc_parts.join(" · ")),
                ..Default::default()
            },
            relations: vec![],
        }))
    }

    // --- Hierarchy ---

    async fn fetch_full_album_detail(
        &self,
        album_id: u64,
    ) -> Option<MetaFullAlbumDetail> {
        let resp = self
            .client
            .get(format!("{}/album/{}", BASE, album_id))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            warn!(album_id, status = %resp.status(), "Deezer album detail HTTP error, skipping");
            return None;
        }
        match resp
            .json::<DeezerResponse<MetaFullAlbumDetail>>()
            .await
            .ok()?
        {
            DeezerResponse::Ok(detail) => Some(detail),
            DeezerResponse::Err { error } => {
                warn!(album_id, %error, "Deezer album detail returned error, skipping");
                None
            }
        }
    }

    fn build_album_children(
        detail: MetaFullAlbumDetail,
        album_id: Uuid,
        album_title: String,
        artist_id: Option<Uuid>,
        artist_title: String,
    ) -> Vec<db::Media> {
        let released_at = detail.release_date.as_deref().and_then(parse_release_date);
        detail
            .tracks
            .data
            .into_iter()
            .map(|track| {
                let track_artist = if track.artist.name.is_empty() {
                    artist_title.clone()
                } else {
                    track.artist.name
                };
                db::Media {
                    id: common::get_stable_uuid(format!("deezer-track:{}", track.id)),
                    title: track.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(track.id.to_string()),
                    poster: detail.cover_xl.clone(),
                    runtime: track.duration.map(|s| s as i64),
                    released_at,
                    description: Some(format!("by {}", track_artist)),
                    idx: track.track_position,
                    parent_idx: track.disk_number,
                    parent_id: Some(album_id),
                    series_id: artist_id,
                    parent_title: Some(album_title.clone()),
                    series_title: Some(artist_title.clone()),
                    ..Default::default()
                }
            })
            .collect()
    }

    async fn sync_artist_children(&self, root: &db::Media) -> Result<Vec<db::Media>> {
        let Some(artist_id) = root.media_id.as_deref() else {
            return Ok(vec![]);
        };
        if artist_id.parse::<u64>().is_err() {
            return Ok(vec![]);
        }

        let artist_resp: MetaArtistDetail = self
            .client
            .get(format!("{}/artist/{}", BASE, artist_id))
            .send()
            .await?
            .json()
            .await?;

        let artist_title = if root.title.is_empty() {
            artist_resp.name
        } else {
            root.title.clone()
        };
        let artist_poster = root.poster.clone().or(artist_resp.picture_xl);

        let albums_resp: MetaArtistAlbumList = match self
            .client
            .get(format!("{}/artist/{}/albums?limit=1000", BASE, artist_id))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
            _ => {
                warn!(
                    artist_id,
                    "Deezer artist albums request failed, returning empty"
                );
                MetaArtistAlbumList { data: vec![] }
            }
        };

        let album_futs = albums_resp.data.into_iter().map(|album| {
            let artist_title = artist_title.clone();
            let artist_poster = artist_poster.clone();
            let root_id = root.id;
            async move {
                let detail = self.fetch_full_album_detail(album.id).await?;

                let released_at =
                    detail.release_date.as_deref().and_then(parse_release_date);
                let genre_names = detail
                    .genres
                    .as_ref()
                    .map(|g| g.data.iter().map(|g| g.name.clone()).collect::<Vec<_>>())
                    .unwrap_or_default();
                let mut desc_parts = vec![format!(
                    "by {}",
                    detail
                        .artist
                        .as_ref()
                        .map(|a| a.name.as_str())
                        .unwrap_or(artist_title.as_str())
                )];
                if !genre_names.is_empty() {
                    desc_parts.push(genre_names.join(", "));
                }
                if let Some(label) = &detail.label {
                    desc_parts.push(label.clone());
                }

                let album_media = db::Media {
                    id: common::get_stable_uuid(format!("deezer-album:{}", detail.id)),
                    title: detail.title.clone(),
                    kind: db::MediaKind::Album,
                    media_id: Some(detail.id.to_string()),
                    poster: detail.cover_xl.clone().or(artist_poster.clone()),
                    released_at,
                    description: Some(desc_parts.join(" · ")),
                    parent_id: Some(root_id),
                    series_id: Some(root_id),
                    series_title: Some(artist_title.clone()),
                    ..Default::default()
                };

                let tracks = Self::build_album_children(
                    detail,
                    album_media.id,
                    album_media.title.clone(),
                    Some(root_id),
                    artist_title,
                );

                Some((album_media, tracks))
            }
        });

        let mut children = Vec::new();
        let albums_with_tracks: Vec<_> = stream::iter(album_futs)
            .buffer_unordered(3)
            .filter_map(|result| async move { result })
            .collect()
            .await;

        for (album, tracks) in albums_with_tracks {
            children.push(album);
            children.extend(tracks);
        }

        Ok(children)
    }

    async fn sync_album_children(&self, root: &db::Media) -> Result<Vec<db::Media>> {
        let Some(album_id) = root.media_id.as_deref() else {
            return Ok(vec![]);
        };
        let Ok(album_id_num) = album_id.parse::<u64>() else {
            return Ok(vec![]);
        };

        let Some(detail) = self.fetch_full_album_detail(album_id_num).await else {
            return Ok(vec![]);
        };

        let album_title = if root.title.is_empty() {
            detail.title.clone()
        } else {
            root.title.clone()
        };
        let artist_title = root.series_title.clone().unwrap_or_else(|| {
            detail
                .artist
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_default()
        });

        Ok(Self::build_album_children(
            detail,
            root.id,
            album_title,
            root.series_id.or(root.parent_id),
            artist_title,
        ))
    }

    // --- Search ---

    async fn search_tracks(
        &self,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer track search starting");

        let url = format!(
            "{}/search?q={}&limit={}",
            BASE,
            urlencoding::encode(query),
            limit.min(50)
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(query, status = %resp.status(), "Deezer track search HTTP error");
            return Ok(vec![]);
        }
        let data: TrackSearchResult = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(|t| track_to_result(t))
            .map(|r| {
                if let Some(ref album) = r.album {
                    let album_result = MusicSearchResult {
                        media: album.clone(),
                        album: None,
                        artist: r.artist.clone(),
                    };
                    ctx.store.insert(
                        album.id.to_string(),
                        album_result,
                        Duration::from_secs(3600),
                    );
                }
                ctx.store.insert(
                    r.media.id.to_string(),
                    r.clone(),
                    Duration::from_secs(3600),
                );
                r.media
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer track search done"
        );
        Ok(results)
    }

    async fn search_albums(
        &self,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer album search starting");

        let url = format!(
            "{}/search/album?q={}&limit={}",
            BASE,
            urlencoding::encode(query),
            limit.min(25)
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(query, status = %resp.status(), "Deezer album search HTTP error");
            return Ok(vec![]);
        }
        let data: AlbumSearchResult = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(|a| album_to_result(a))
            .map(|r| {
                ctx.store.insert(
                    r.media.id.to_string(),
                    r.clone(),
                    Duration::from_secs(3600),
                );
                r.media
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer album search done"
        );
        Ok(results)
    }

    async fn search_artists(
        &self,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer artist search starting");

        let url = format!(
            "{}/search/artist?q={}&limit={}",
            BASE,
            urlencoding::encode(query),
            limit.min(25)
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(query, status = %resp.status(), "Deezer artist search HTTP error");
            return Ok(vec![]);
        }
        let data: SearchArtistList = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(|a| {
                let artist = db::Media {
                    id: common::get_stable_uuid(format!("deezer-artist:{}", a.id)),
                    title: a.name,
                    kind: db::MediaKind::Artist,
                    media_id: Some(a.id.to_string()),
                    poster: a.picture_xl,
                    ..Default::default()
                };
                let result = MusicSearchResult {
                    media: artist,
                    album: None,
                    artist: None,
                };
                ctx.store.insert(
                    result.media.id.to_string(),
                    result.clone(),
                    Duration::from_secs(3600),
                );
                result.media
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer artist search done"
        );
        Ok(results)
    }

    async fn persist_track(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let result = match ctx.store.get::<MusicSearchResult>(id.to_string()) {
            Some(r) if r.media.kind == db::MediaKind::Track => r,
            _ => return Ok(None),
        };
        ctx.store.delete(id.to_string());
        let target_id = result.media.id;
        let artist_deezer_id = result
            .artist
            .as_ref()
            .and_then(|a| a.media_id.clone())
            .ok_or_else(|| anyhow!("Track has no artist Deezer ID"))?;
        let t = std::time::Instant::now();
        tracing::debug!(artist_id = %artist_deezer_id, "Fetching full artist discography for track persist");
        let (artist, albums_with_tracks) =
            self.fetch_full_discography(&artist_deezer_id).await?;
        tracing::debug!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );
        save_discography(artist, albums_with_tracks, ctx).await?;
        Ok(db::Media::get_by_id(&ctx.db, &target_id).await?)
    }

    async fn persist_album(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let result = match ctx.store.get::<MusicSearchResult>(id.to_string()) {
            Some(r) if r.media.kind == db::MediaKind::Album => r,
            _ => return Ok(None),
        };
        ctx.store.delete(id.to_string());
        let target_id = result.media.id;
        let artist_deezer_id = result
            .artist
            .as_ref()
            .and_then(|a| a.media_id.clone())
            .ok_or_else(|| anyhow!("Album has no artist Deezer ID"))?;
        let t = std::time::Instant::now();
        tracing::debug!(artist_id = %artist_deezer_id, "Fetching full artist discography for album persist");
        let (artist, albums_with_tracks) =
            self.fetch_full_discography(&artist_deezer_id).await?;
        tracing::debug!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );
        save_discography(artist, albums_with_tracks, ctx).await?;
        Ok(db::Media::get_by_id(&ctx.db, &target_id).await?)
    }

    async fn persist_artist(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let result = match ctx.store.get::<MusicSearchResult>(id.to_string()) {
            Some(r) if r.media.kind == db::MediaKind::Artist => r,
            _ => return Ok(None),
        };
        ctx.store.delete(id.to_string());
        let artist_deezer_id = result
            .media
            .media_id
            .clone()
            .ok_or_else(|| anyhow!("Artist has no Deezer ID"))?;
        let t = std::time::Instant::now();
        tracing::debug!(artist_id = %artist_deezer_id, "Fetching full artist discography for artist persist");
        let (artist, albums_with_tracks) =
            self.fetch_full_discography(&artist_deezer_id).await?;
        let target_id = artist.id;
        tracing::debug!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );
        save_discography(artist, albums_with_tracks, ctx).await?;
        Ok(db::Media::get_by_id(&ctx.db, &target_id).await?)
    }

    async fn fetch_full_discography(
        &self,
        artist_id: &str,
    ) -> Result<(db::Media, Vec<(db::Media, Vec<db::Media>)>)> {
        let artist_resp: SearchArtistDetail = self
            .client
            .get(format!("{}/artist/{}", BASE, artist_id))
            .send()
            .await?
            .json()
            .await?;

        let artist = db::Media {
            id: common::get_stable_uuid(format!("deezer-artist:{}", artist_resp.id)),
            title: artist_resp.name.clone(),
            kind: db::MediaKind::Artist,
            media_id: Some(artist_resp.id.to_string()),
            poster: artist_resp.picture_xl,
            ..Default::default()
        };

        let albums_resp: SearchAlbumList = match self
            .client
            .get(format!("{}/artist/{}/albums?limit=1000", BASE, artist_id))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
            _ => {
                warn!(artist_id, "Deezer /artist/albums request failed");
                SearchAlbumList { data: vec![] }
            }
        };

        const CONCURRENCY: usize = 3;
        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY: Duration = Duration::from_secs(1);

        let artist_name = artist_resp.name.clone();
        let album_futs = albums_resp.data.into_iter().map(|a| {
            let client = self.client.clone();
            let artist_name = artist_name.clone();
            async move {
                let url = format!("{}/album/{}", BASE, a.id);
                let mut attempt = 0u32;
                let detail: SearchAlbumDetail = loop {
                    attempt += 1;
                    let resp = match client.get(&url).send().await {
                        Ok(r) if r.status().is_success() => r,
                        Ok(r) => {
                            warn!(album_id = a.id, status = %r.status(), "Deezer album detail HTTP error, skipping");
                            return None;
                        }
                        Err(e) => {
                            warn!(album_id = a.id, error = %e, "Deezer album request failed, skipping");
                            return None;
                        }
                    };
                    match resp
                        .json::<DeezerDiscographyResponse<SearchAlbumDetail>>()
                        .await
                    {
                        Ok(DeezerDiscographyResponse::Ok(d)) => break d,
                        Ok(DeezerDiscographyResponse::Err { error }) => {
                            let is_rate_limit = error
                                .get("code")
                                .and_then(|v| v.as_i64())
                                == Some(4);
                            if is_rate_limit && attempt <= MAX_RETRIES {
                                tracing::debug!(album_id = a.id, attempt, "Deezer rate limit, retrying");
                                tokio::time::sleep(RETRY_DELAY * attempt).await;
                                continue;
                            }
                            warn!(album_id = a.id, %error, "Deezer album detail error, skipping");
                            return None;
                        }
                        Err(e) => {
                            warn!(album_id = a.id, error = %e, "Deezer album detail parse error, skipping");
                            return None;
                        }
                    }
                };

                let released_at = detail
                    .release_date
                    .as_deref()
                    .and_then(|s| {
                        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                            .ok()
                            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                    });

                let genre_names: Vec<String> = detail
                    .genres
                    .as_ref()
                    .map(|g| g.data.iter().map(|g| g.name.clone()).collect())
                    .unwrap_or_default();

                let artist_name_for_desc = detail
                    .artist
                    .as_ref()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| artist_name.clone());

                let mut desc_parts = vec![format!("by {}", artist_name_for_desc)];
                if !genre_names.is_empty() {
                    desc_parts.push(genre_names.join(", "));
                }
                if let Some(label) = &detail.label {
                    desc_parts.push(label.clone());
                }

                let album = db::Media {
                    id: common::get_stable_uuid(format!("deezer-album:{}", detail.id)),
                    title: detail.title.clone(),
                    kind: db::MediaKind::Album,
                    media_id: Some(detail.id.to_string()),
                    poster: detail.cover_xl.clone(),
                    released_at,
                    description: Some(desc_parts.join(" · ")),
                    ..Default::default()
                };

                let tracks: Vec<db::Media> = detail
                    .tracks
                    .data
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let track_artist = if t.artist.name.is_empty() {
                            artist_name.clone()
                        } else {
                            t.artist.name
                        };
                        db::Media {
                            id: common::get_stable_uuid(format!(
                                "deezer-track:{}",
                                t.id
                            )),
                            title: t.title,
                            kind: db::MediaKind::Track,
                            media_id: Some(t.id.to_string()),
                            poster: detail.cover_xl.clone(),
                            runtime: t.duration.map(|s| s as i64),
                            released_at,
                            description: Some(format!("by {}", track_artist)),
                            idx: Some(i as i64 + 1),
                            parent_idx: t.disk_number,
                            ..Default::default()
                        }
                    })
                    .collect();

                Some((album, tracks))
            }
        });

        let albums_with_tracks: Vec<_> = futures::stream::iter(album_futs)
            .buffer_unordered(CONCURRENCY)
            .filter_map(|r| async move { r })
            .collect()
            .await;

        tracing::debug!(
            artist_id,
            artist = %artist_resp.name,
            albums = albums_with_tracks.len(),
            "Fetched full discography"
        );

        Ok((artist, albums_with_tracks))
    }

    // --- Catalog (playlist) ---

    async fn fetch_playlist_stream(
        &self,
        ctx: &AppContext,
        playlist_id: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>> {
        let url = format!("{}/playlist/{}", BASE, playlist_id);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "Deezer playlist {} returned {}",
                playlist_id,
                resp.status()
            ));
        }
        let playlist = match resp.json::<DeezerResponse<PlaylistResponse>>().await? {
            DeezerResponse::Ok(p) => p,
            DeezerResponse::Err { error } => {
                return Err(anyhow!("Deezer playlist error: {}", error));
            }
        };

        let items: Vec<db::Media> = playlist
            .tracks
            .data
            .into_iter()
            .map(|track| {
                let released_at = track
                    .album
                    .release_date
                    .as_deref()
                    .and_then(parse_release_date);
                db::Media {
                    id: common::get_stable_uuid(format!("deezer-track:{}", track.id)),
                    title: track.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(track.id.to_string()),
                    poster: track.album.cover_xl,
                    runtime: Some(track.duration as i64),
                    released_at,
                    description: Some(format!("by {}", track.artist.name)),
                    ..Default::default()
                }
            })
            .collect();

        Ok(Box::pin(futures::stream::iter(items)))
    }
}

// ---------------------------------------------------------------------------
// AddonKind impl
// ---------------------------------------------------------------------------

#[async_trait]
impl AddonKind for DeezerAddon {
    fn id(&self) -> &'static str {
        "deezer"
    }

    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(self
            .playlists()
            .iter()
            .map(|id| CatalogInfo {
                provider_catalog_id: id.clone(),
                name: format!("Deezer playlist {id}"),
            })
            .collect())
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        if !self.playlists().iter().any(|id| id == local_id) {
            return Ok(None);
        }
        Ok(Some(self.fetch_playlist_stream(ctx, local_id).await?))
    }

    async fn meta_supports(&self, media: &db::Media) -> bool {
        self.meta_can_refresh(media)
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        let deezer_id = match &media.media_id {
            Some(id) => id.clone(),
            None => return Ok(None),
        };
        match media.kind {
            db::MediaKind::Track => self.fetch_track_meta(&deezer_id, media).await,
            db::MediaKind::Album => self.fetch_album_meta(&deezer_id, media).await,
            _ => Ok(None),
        }
    }

    fn hierarchy_supports(&self, root: &db::Media) -> bool {
        matches!(root.kind, db::MediaKind::Artist | db::MediaKind::Album)
    }

    async fn hierarchy_sync_children(
        &self,
        root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if !self.hierarchy_supports(root) {
            return Ok(None);
        }
        let children = match root.kind {
            db::MediaKind::Artist => self.sync_artist_children(root).await?,
            db::MediaKind::Album => self.sync_album_children(root).await?,
            _ => return Ok(None),
        };
        if children.is_empty() {
            Ok(None)
        } else {
            Ok(Some(children))
        }
    }

    async fn search_supports(&self, kind: &db::MediaKind) -> bool {
        matches!(
            kind,
            db::MediaKind::Track | db::MediaKind::Album | db::MediaKind::Artist
        )
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        match kind {
            db::MediaKind::Track => {
                Ok(Some(self.search_tracks(query, limit, ctx).await?))
            }
            db::MediaKind::Album => {
                Ok(Some(self.search_albums(query, limit, ctx).await?))
            }
            db::MediaKind::Artist => {
                Ok(Some(self.search_artists(query, limit, ctx).await?))
            }
            _ => Ok(None),
        }
    }

    async fn search_persist(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        if let Some(m) = self.persist_track(id, ctx).await? {
            return Ok(Some(m));
        }
        if let Some(m) = self.persist_album(id, ctx).await? {
            return Ok(Some(m));
        }
        self.persist_artist(id, ctx).await
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn extract_playlist_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }
    trimmed.split('/').last().and_then(|s| {
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
            Some(s.to_string())
        } else {
            None
        }
    })
}

fn track_to_result(t: SearchTrack) -> MusicSearchResult {
    let album = db::Media {
        id: common::get_stable_uuid(format!("deezer-album:{}", t.album.id)),
        title: t.album.title.clone(),
        kind: db::MediaKind::Album,
        media_id: Some(t.album.id.to_string()),
        poster: t.album.cover_medium.clone(),
        description: Some(format!("by {}", t.artist.name)),
        ..Default::default()
    };
    let artist = db::Media {
        id: common::get_stable_uuid(format!("deezer-artist:{}", t.artist.id)),
        title: t.artist.name.clone(),
        kind: db::MediaKind::Artist,
        media_id: Some(t.artist.id.to_string()),
        poster: t.artist.picture_medium.clone(),
        ..Default::default()
    };
    let track = db::Media {
        id: common::get_stable_uuid(format!("deezer-track:{}", t.id)),
        title: t.title,
        kind: db::MediaKind::Track,
        media_id: Some(t.id.to_string()),
        poster: t.album.cover_medium,
        runtime: t.duration.map(|s| s as i64),
        description: Some(format!("by {}", t.artist.name)),
        parent_id: Some(album.id),
        parent_title: Some(t.album.title),
        series_title: Some(t.artist.name),
        ..Default::default()
    };
    MusicSearchResult {
        media: track,
        album: Some(album),
        artist: Some(artist),
    }
}

fn album_to_result(a: SearchAlbum) -> MusicSearchResult {
    let artist = db::Media {
        id: common::get_stable_uuid(format!("deezer-artist:{}", a.artist.id)),
        title: a.artist.name.clone(),
        kind: db::MediaKind::Artist,
        media_id: Some(a.artist.id.to_string()),
        poster: a.artist.picture_medium.clone(),
        ..Default::default()
    };
    let album = db::Media {
        id: common::get_stable_uuid(format!("deezer-album:{}", a.id)),
        title: a.title,
        kind: db::MediaKind::Album,
        media_id: Some(a.id.to_string()),
        poster: a.cover_medium,
        description: Some(format!("by {}", a.artist.name)),
        series_title: Some(a.artist.name),
        ..Default::default()
    };
    MusicSearchResult {
        media: album,
        album: None,
        artist: Some(artist),
    }
}

async fn save_discography(
    mut artist: db::Media,
    albums_with_tracks: Vec<(db::Media, Vec<db::Media>)>,
    ctx: &AppContext,
) -> Result<()> {
    artist.save(&ctx.db).await.ok();
    let artist_id = artist.id;
    for (mut album, mut tracks) in albums_with_tracks {
        album.parent_id = Some(artist_id);
        album.series_id = Some(artist_id);
        album.series_title = Some(artist.title.clone());
        album.save(&ctx.db).await.ok();
        let album_id = album.id;
        for track in &mut tracks {
            track.parent_id = Some(album_id);
            track.series_id = Some(artist_id);
            track.series_title = Some(artist.title.clone());
            track.save(&ctx.db).await.ok();
        }
    }
    Ok(())
}
