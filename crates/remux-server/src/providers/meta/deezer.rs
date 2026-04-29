use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::stream::{self, StreamExt};
use serde::Deserialize;

use crate::providers::{HierarchySyncProvider, MetaProvider, MetaResult};

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

/// Deezer returns 200 OK with `{"error": {...}}` for missing/unavailable items.
/// This wrapper lets us detect that and return `Ok(None)` gracefully.
#[derive(Deserialize)]
#[serde(untagged)]
enum DeezerResponse<T> {
    Ok(T),
    Err { error: serde_json::Value },
}

#[derive(Deserialize)]
struct TrackDetail {
    id: u64,
    title: String,
    duration: u64,
    release_date: Option<String>,
    isrc: Option<String>,
    artist: TrackArtist,
    album: TrackAlbum,
}

#[derive(Deserialize)]
struct TrackArtist {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize)]
struct TrackAlbum {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct AlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<GenreList>,
    artist: Option<AlbumArtist>,
    #[serde(default)]
    nb_tracks: u32,
}

#[derive(Deserialize)]
struct GenreList {
    data: Vec<Genre>,
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
struct AlbumArtist {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize)]
struct ArtistDetail {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize, Default)]
struct ArtistAlbumList {
    data: Vec<ArtistAlbumSummary>,
}

#[derive(Deserialize)]
struct ArtistAlbumSummary {
    id: u64,
}

#[derive(Deserialize)]
struct ArtistRef {
    name: String,
}

#[derive(Deserialize, Default)]
struct AlbumTrackList {
    data: Vec<AlbumTrackSummary>,
}

#[derive(Deserialize)]
struct AlbumTrackSummary {
    id: u64,
    title: String,
    duration: Option<u64>,
    track_position: Option<i64>,
    disk_number: Option<i64>,
    artist: TrackArtist,
}

#[derive(Deserialize)]
struct FullAlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<GenreList>,
    artist: Option<ArtistRef>,
    #[serde(default)]
    tracks: AlbumTrackList,
}

/// Music metadata provider backed by the Deezer public API.
///
/// Fetches full track/album details by Deezer ID (`media.media_id`).
/// No API key required.
pub struct DeezerMusicMetaProvider {
    client: reqwest::Client,
}

impl Default for DeezerMusicMetaProvider {
    fn default() -> Self {
        Self {
            client: build_client(),
        }
    }
}

pub struct DeezerHierarchySyncProvider {
    client: reqwest::Client,
}

impl Default for DeezerHierarchySyncProvider {
    fn default() -> Self {
        Self {
            client: build_client(),
        }
    }
}

#[async_trait]
impl MetaProvider for DeezerMusicMetaProvider {
    fn supported_kinds(&self) -> &'static [db::MediaKind] {
        &[db::MediaKind::Track, db::MediaKind::Album]
    }

    fn can_refresh(&self, media: &db::Media) -> bool {
        self.supports(&media.kind)
            && media
                .media_id
                .as_deref()
                .is_some_and(|id| id.parse::<u64>().is_ok())
    }

    async fn fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        let deezer_id = match &media.media_id {
            Some(id) => id.clone(),
            None => return Ok(None),
        };

        match media.kind {
            db::MediaKind::Track => self.fetch_track(&deezer_id, media).await,
            db::MediaKind::Album => self.fetch_album(&deezer_id, media).await,
            _ => Ok(None),
        }
    }
}

impl DeezerMusicMetaProvider {
    async fn fetch_track(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<MetaResult>> {
        let url = format!("{}/track/{}", BASE, deezer_id);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            tracing::warn!(deezer_id, status = %resp.status(), "Deezer track detail HTTP error");
            return Ok(None);
        }

        let t: TrackDetail = match resp.json::<DeezerResponse<TrackDetail>>().await? {
            DeezerResponse::Ok(t) => t,
            DeezerResponse::Err { error } => {
                tracing::warn!(deezer_id, %error, "Deezer track detail returned error");
                return Ok(None);
            }
        };

        let released_at = t
            .release_date
            .as_deref()
            .or(t.album.release_date.as_deref())
            .and_then(parse_release_date);

        let media = db::Media {
            id: base.id,
            title: t.title,
            kind: db::MediaKind::Track,
            media_id: Some(t.id.to_string()),
            poster: t.album.cover_xl,
            runtime: Some(t.duration as i64),
            released_at,
            description: Some(format!("by {}", t.artist.name)),
            ..Default::default()
        };

        tracing::debug!(deezer_id, "Deezer track detail fetched");
        Ok(Some(MetaResult {
            media,
            relations: vec![],
        }))
    }

    async fn fetch_album(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<MetaResult>> {
        let url = format!("{}/album/{}", BASE, deezer_id);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            tracing::warn!(deezer_id, status = %resp.status(), "Deezer album detail HTTP error");
            return Ok(None);
        }

        let a: AlbumDetail = match resp.json::<DeezerResponse<AlbumDetail>>().await? {
            DeezerResponse::Ok(a) => a,
            DeezerResponse::Err { error } => {
                tracing::warn!(deezer_id, %error, "Deezer album detail returned error");
                return Ok(None);
            }
        };

        let released_at = a.release_date.as_deref().and_then(parse_release_date);

        let genre_names = a
            .genres
            .as_ref()
            .map(|g| g.data.iter().map(|g| g.name.clone()).collect::<Vec<_>>());

        // Build a description: "by {artist} · {genre} · {label}"
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

        let media = db::Media {
            id: base.id,
            title: a.title,
            kind: db::MediaKind::Album,
            media_id: Some(a.id.to_string()),
            poster: a.cover_xl,
            released_at,
            description: Some(desc_parts.join(" · ")),
            ..Default::default()
        };

        tracing::debug!(
            deezer_id,
            nb_tracks = a.nb_tracks,
            "Deezer album detail fetched"
        );
        Ok(Some(MetaResult {
            media,
            relations: vec![],
        }))
    }
}

#[async_trait]
impl HierarchySyncProvider for DeezerHierarchySyncProvider {
    fn supported_root_kinds(&self) -> &'static [db::MediaKind] {
        &[db::MediaKind::Artist, db::MediaKind::Album]
    }

    async fn sync_children(
        &self,
        root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        match root.kind {
            db::MediaKind::Artist => self.sync_artist_children(root).await,
            db::MediaKind::Album => self.sync_album_children(root).await,
            _ => Ok(vec![]),
        }
    }
}

impl DeezerHierarchySyncProvider {
    async fn fetch_album_detail(&self, album_id: u64) -> Option<FullAlbumDetail> {
        let resp = self
            .client
            .get(format!("{}/album/{}", BASE, album_id))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            tracing::warn!(
                album_id,
                status = %resp.status(),
                "Deezer album detail HTTP error, skipping"
            );
            return None;
        }

        match resp.json::<DeezerResponse<FullAlbumDetail>>().await.ok()? {
            DeezerResponse::Ok(detail) => Some(detail),
            DeezerResponse::Err { error } => {
                tracing::warn!(album_id, %error, "Deezer album detail returned error, skipping");
                None
            }
        }
    }

    fn build_album_children(
        detail: FullAlbumDetail,
        album_id: uuid::Uuid,
        album_title: String,
        artist_id: Option<uuid::Uuid>,
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
                    id: crate::common::get_stable_uuid(format!(
                        "deezer-track:{}",
                        track.id
                    )),
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

        let artist_resp: ArtistDetail = self
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

        let albums_resp: ArtistAlbumList = match self
            .client
            .get(format!("{}/artist/{}/albums?limit=1000", BASE, artist_id))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
            _ => {
                tracing::warn!(
                    artist_id,
                    "Deezer artist albums request failed, returning empty"
                );
                ArtistAlbumList { data: vec![] }
            }
        };

        let album_futs = albums_resp.data.into_iter().map(|album| {
            let artist_title = artist_title.clone();
            let artist_poster = artist_poster.clone();
            let root_id = root.id;
            async move {
                let detail = self.fetch_album_detail(album.id).await?;

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
                    id: crate::common::get_stable_uuid(format!(
                        "deezer-album:{}",
                        detail.id
                    )),
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

        let Some(detail) = self.fetch_album_detail(album_id_num).await else {
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
                .map(|artist| artist.name.clone())
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
}
