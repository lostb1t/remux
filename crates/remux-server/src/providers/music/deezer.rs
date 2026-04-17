use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use chrono::NaiveDateTime;
use serde::Deserialize;

use super::{MusicMetaProvider, MusicMetaResult};

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


/// Music metadata provider backed by the Deezer public API.
///
/// Fetches full track/album details by Deezer ID (`media.media_id`).
/// No API key required.
pub struct DeezerMusicMetaProvider {
    client: reqwest::Client,
}

impl Default for DeezerMusicMetaProvider {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[async_trait]
impl MusicMetaProvider for DeezerMusicMetaProvider {
    async fn fetch(&self, media: &db::Media, _ctx: &AppContext) -> Result<Option<MusicMetaResult>> {
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
    ) -> Result<Option<MusicMetaResult>> {
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
        Ok(Some(MusicMetaResult { media }))
    }

    async fn fetch_album(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<MusicMetaResult>> {
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

        tracing::debug!(deezer_id, nb_tracks = a.nb_tracks, "Deezer album detail fetched");
        Ok(Some(MusicMetaResult { media }))
    }
}
