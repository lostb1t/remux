use std::time::Duration;

use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt as _;
use serde::Deserialize;
use uuid::Uuid;

use super::{MusicSearchResult, SearchService};

const BASE: &str = "https://api.deezer.com";

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .build()
        .expect("failed to build HTTP client")
}


#[derive(Deserialize)]
struct DeezerArtist {
    id: u64,
    name: String,
    picture_medium: Option<String>,
}

#[derive(Deserialize)]
struct ArtistSearch {
    data: Vec<DeezerArtistSearchItem>,
}

#[derive(Deserialize)]
struct DeezerArtistSearchItem {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize)]
struct DeezerAlbumRef {
    id: u64,
    title: String,
    cover_medium: Option<String>,
}


#[derive(Deserialize)]
struct DeezerArtistDetail {
    id: u64,
    name: String,
    picture_xl: Option<String>,
}

#[derive(Deserialize, Default)]
struct DeezerAlbumList {
    data: Vec<DeezerAlbumSummary>,
}

#[derive(Deserialize)]
struct DeezerAlbumSummary {
    id: u64,
}

/// Full album detail returned by `GET /album/{id}` — includes tracks.
#[derive(Deserialize)]
struct DeezerAlbumDetail {
    id: u64,
    title: String,
    cover_xl: Option<String>,
    release_date: Option<String>,
    label: Option<String>,
    genres: Option<DeezerGenreList>,
    artist: Option<DeezerArtistRef>,
    #[serde(default)]
    tracks: DeezerTrackList,
}

#[derive(Deserialize, Default)]
struct DeezerTrackList {
    data: Vec<DeezerTrackSummary>,
}

#[derive(Deserialize)]
struct DeezerGenreList {
    data: Vec<DeezerGenre>,
}

#[derive(Deserialize)]
struct DeezerGenre {
    name: String,
}

#[derive(Deserialize)]
struct DeezerArtistRef {
    name: String,
}

#[derive(Deserialize)]
struct DeezerTrackSummary {
    id: u64,
    title: String,
    duration: Option<u64>,
    track_position: Option<i64>,
    disk_number: Option<i64>,
    artist: DeezerArtist,
}

/// Deezer returns 200 OK with `{"error": {...}}` for unavailable items.
#[derive(Deserialize)]
#[serde(untagged)]
enum DeezerDiscographyResponse<T> {
    Ok(T),
    Err { error: serde_json::Value },
}


#[derive(Deserialize)]
struct TrackSearch {
    data: Vec<DeezerTrack>,
}

#[derive(Deserialize)]
struct DeezerTrack {
    id: u64,
    title: String,
    /// Duration in seconds.
    duration: Option<u64>,
    artist: DeezerArtist,
    album: DeezerAlbumRef,
}

fn track_to_result(t: DeezerTrack) -> MusicSearchResult {
    let album = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-album:{}", t.album.id)),
        title: t.album.title.clone(),
        kind: db::MediaKind::Album,
        media_id: Some(t.album.id.to_string()),
        poster: t.album.cover_medium.clone(),
        description: Some(format!("by {}", t.artist.name)),
        ..Default::default()
    };
    let artist = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-artist:{}", t.artist.id)),
        title: t.artist.name.clone(),
        kind: db::MediaKind::Artist,
        media_id: Some(t.artist.id.to_string()),
        poster: t.artist.picture_medium.clone(),
        ..Default::default()
    };
    let track = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-track:{}", t.id)),
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
    MusicSearchResult { media: track, album: Some(album), artist: Some(artist) }
}


/// Fetch the full discography for a Deezer artist.
///
/// Makes **N+2** requests total (artist + album list + one `GET /album/{id}` per album),
/// compared to the previous 2N+2 approach (separate tracks + apply_meta calls).
/// Each `GET /album/{id}` response includes both metadata (genres, label, cover)
/// AND the track listing — so no separate track-list or apply_meta call is needed.
async fn fetch_full_discography(
    client: &reqwest::Client,
    artist_id: &str,
) -> Result<(db::Media, Vec<(db::Media, Vec<db::Media>)>)> {
    let artist_resp: DeezerArtistDetail =
        client.get(format!("{}/artist/{}", BASE, artist_id)).send().await?.json().await?;

    let artist = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-artist:{}", artist_resp.id)),
        title: artist_resp.name.clone(),
        kind: db::MediaKind::Artist,
        media_id: Some(artist_resp.id.to_string()),
        poster: artist_resp.picture_xl,
        ..Default::default()
    };

    let albums_resp: DeezerAlbumList = match client
        .get(format!("{}/artist/{}/albums?limit=100", BASE, artist_id))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => {
            tracing::warn!(artist_id, "Deezer /artist/albums request failed, returning empty");
            DeezerAlbumList { data: vec![] }
        }
    };

    // Deezer's public API rate-limits hard; we retry up to 3 times with 1s backoff
    // on quota errors before skipping an album.
    const CONCURRENCY: usize = 3;
    const MAX_RETRIES: u32 = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(1);

    let artist_name = artist_resp.name.clone();

    let album_futs = albums_resp.data.into_iter().map(|a| {
        let client = client.clone();
        let artist_name = artist_name.clone();
        async move {
            let url = format!("{}/album/{}", BASE, a.id);

            let mut attempt = 0u32;
            let detail: DeezerAlbumDetail = loop {
                attempt += 1;
                let resp = match client.get(&url).send().await {
                    Ok(r) if r.status().is_success() => r,
                    Ok(r) => {
                        tracing::warn!(album_id = a.id, status = %r.status(), "Deezer album detail HTTP error, skipping");
                        return None;
                    }
                    Err(e) => {
                        tracing::warn!(album_id = a.id, error = %e, "Deezer album detail request failed, skipping");
                        return None;
                    }
                };

                match resp.json::<DeezerDiscographyResponse<DeezerAlbumDetail>>().await {
                    Ok(DeezerDiscographyResponse::Ok(d)) => break d,
                    Ok(DeezerDiscographyResponse::Err { error }) => {
                        // code 4 = quota exceeded — retry with backoff
                        let is_rate_limit = error.get("code").and_then(|v| v.as_i64()) == Some(4);
                        if is_rate_limit && attempt <= MAX_RETRIES {
                            tracing::debug!(album_id = a.id, attempt, "Deezer rate limit, retrying after delay");
                            tokio::time::sleep(RETRY_DELAY * attempt).await;
                            continue;
                        }
                        tracing::warn!(album_id = a.id, %error, "Deezer album detail returned error, skipping");
                        return None;
                    }
                    Err(e) => {
                        tracing::warn!(album_id = a.id, error = %e, "Deezer album detail parse error, skipping");
                        return None;
                    }
                }
            };

            let released_at = detail
                .release_date
                .as_deref()
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap());

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
                id: crate::utils::get_stable_uuid(format!("deezer-album:{}", detail.id)),
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
                .map(|t| {
                    let track_artist =
                        if t.artist.name.is_empty() { artist_name.clone() } else { t.artist.name };
                    db::Media {
                        id: crate::utils::get_stable_uuid(format!("deezer-track:{}", t.id)),
                        title: t.title,
                        kind: db::MediaKind::Track,
                        media_id: Some(t.id.to_string()),
                        poster: detail.cover_xl.clone(),
                        runtime: t.duration.map(|s| s as i64),
                        released_at,
                        description: Some(format!("by {}", track_artist)),
                        idx: t.track_position,
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

    tracing::info!(
        artist_id,
        artist = %artist_resp.name,
        albums = albums_with_tracks.len(),
        "Fetched full discography"
    );

    Ok((artist, albums_with_tracks))
}

/// Persist a full discography tree to the database, setting all parent-child links.
///
/// - `album.parent_id = artist.id`
/// - `track.parent_id = album.id`
/// - `track.series_id = artist.id`
///
/// All metadata (genres, label, cover, release date, tracks) was already fetched
/// in `fetch_full_discography` — no additional API calls are made here.
async fn save_discography(
    mut artist: db::Media,
    albums_with_tracks: Vec<(db::Media, Vec<db::Media>)>,
    ctx: &AppContext,
) -> Result<()> {
    artist.save(&ctx.db).await.ok();
    let artist_id = artist.id;

    for (mut album, mut tracks) in albums_with_tracks {
        album.parent_id = Some(artist_id);
        album.save(&ctx.db).await.ok();
        let album_id = album.id;

        for track in &mut tracks {
            track.parent_id = Some(album_id);
            track.series_id = Some(artist_id);
            track.save(&ctx.db).await.ok();
        }
    }

    Ok(())
}

/// Search backend backed by the Deezer public API — handles tracks.
///
/// No API key required. Typically responds in ~100 ms.
pub struct DeezerTrackSearchService {
    client: reqwest::Client,
}

impl Default for DeezerTrackSearchService {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[async_trait]
impl SearchService for DeezerTrackSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn search(
        &self,
        _kind: &db::MediaKind,
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
            tracing::warn!(query, status = %resp.status(), "Deezer track search HTTP error");
            return Ok(vec![]);
        }

        let data: TrackSearch = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(track_to_result)
            .map(|r| {
                // Also cache the album so persist works if the user navigates to it directly.
                if let Some(ref album) = r.album {
                    let album_result = MusicSearchResult {
                        media: album.clone(),
                        album: None,
                        artist: r.artist.clone(),
                    };
                    ctx.store.insert(album.id.to_string(), album_result, Duration::from_secs(3600));
                }
                ctx.store.insert(r.media.id.to_string(), r.clone(), Duration::from_secs(3600));
                r.media
            })
            .collect();

        tracing::info!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer track search done"
        );

        Ok(results)
    }

    async fn persist(&self, id: Uuid, ctx: &AppContext) -> Result<Option<db::Media>> {
        let result = match ctx.store.get::<MusicSearchResult>(id.to_string()) {
            Some(r) if r.media.kind == db::MediaKind::Track => r,
            _ => return Ok(None),
        };
        ctx.store.delete(id.to_string());

        // The item we want to return after saving
        let target_id = result.media.id;

        let artist_deezer_id = result
            .artist
            .as_ref()
            .and_then(|a| a.media_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Track has no artist Deezer ID"))?;

        let t = std::time::Instant::now();
        tracing::info!(artist_id = %artist_deezer_id, "Fetching full artist discography for track persist");
        let (artist, albums_with_tracks) =
            fetch_full_discography(&self.client, &artist_deezer_id).await?;
        tracing::info!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );

        save_discography(artist, albums_with_tracks, ctx).await?;

        // Return the specific track that was clicked.
        let media = db::Media::get_by_id(&ctx.db, &target_id).await?;
        Ok(media)
    }
}


#[derive(Deserialize)]
struct AlbumSearch {
    data: Vec<DeezerAlbum>,
}

#[derive(Deserialize)]
struct DeezerAlbum {
    id: u64,
    title: String,
    cover_medium: Option<String>,
    artist: DeezerArtist,
}

fn album_to_result(a: DeezerAlbum) -> MusicSearchResult {
    let artist = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-artist:{}", a.artist.id)),
        title: a.artist.name.clone(),
        kind: db::MediaKind::Artist,
        media_id: Some(a.artist.id.to_string()),
        poster: a.artist.picture_medium.clone(),
        ..Default::default()
    };
    let album = db::Media {
        id: crate::utils::get_stable_uuid(format!("deezer-album:{}", a.id)),
        title: a.title,
        kind: db::MediaKind::Album,
        media_id: Some(a.id.to_string()),
        poster: a.cover_medium,
        description: Some(format!("by {}", a.artist.name)),
        series_title: Some(a.artist.name),
        ..Default::default()
    };
    MusicSearchResult { media: album, album: None, artist: Some(artist) }
}

/// Search backend backed by the Deezer public API — handles albums.
///
/// No API key required. Typically responds in ~100 ms.
pub struct DeezerAlbumSearchService {
    client: reqwest::Client,
}

impl Default for DeezerAlbumSearchService {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[async_trait]
impl SearchService for DeezerAlbumSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Album]
    }

    async fn search(
        &self,
        _kind: &db::MediaKind,
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
            tracing::warn!(query, status = %resp.status(), "Deezer album search HTTP error");
            return Ok(vec![]);
        }

        let data: AlbumSearch = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(album_to_result)
            .map(|r| {
                ctx.store.insert(r.media.id.to_string(), r.clone(), Duration::from_secs(3600));
                r.media
            })
            .collect();

        tracing::info!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer album search done"
        );

        Ok(results)
    }

    async fn persist(&self, id: Uuid, ctx: &AppContext) -> Result<Option<db::Media>> {
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
            .ok_or_else(|| anyhow::anyhow!("Album has no artist Deezer ID"))?;

        let t = std::time::Instant::now();
        tracing::info!(artist_id = %artist_deezer_id, "Fetching full artist discography for album persist");
        let (artist, albums_with_tracks) =
            fetch_full_discography(&self.client, &artist_deezer_id).await?;
        tracing::info!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );

        save_discography(artist, albums_with_tracks, ctx).await?;

        // Return the specific album that was clicked.
        let media = db::Media::get_by_id(&ctx.db, &target_id).await?;
        Ok(media)
    }
}

/// Search backend backed by the Deezer public API — handles artists.
pub struct DeezerArtistSearchService {
    client: reqwest::Client,
}

impl Default for DeezerArtistSearchService {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[async_trait]
impl SearchService for DeezerArtistSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Artist]
    }

    async fn search(
        &self,
        _kind: &db::MediaKind,
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
            tracing::warn!(query, status = %resp.status(), "Deezer artist search HTTP error");
            return Ok(vec![]);
        }

        let data: ArtistSearch = resp.json().await?;

        let results: Vec<_> = data
            .data
            .into_iter()
            .map(|a| {
                let artist = db::Media {
                    id: crate::utils::get_stable_uuid(format!("deezer-artist:{}", a.id)),
                    title: a.name,
                    kind: db::MediaKind::Artist,
                    media_id: Some(a.id.to_string()),
                    poster: a.picture_xl,
                    ..Default::default()
                };
                let result = MusicSearchResult { media: artist, album: None, artist: None };
                ctx.store.insert(result.media.id.to_string(), result.clone(), Duration::from_secs(3600));
                result.media
            })
            .collect();

        tracing::info!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "Deezer artist search done"
        );

        Ok(results)
    }

    async fn persist(&self, id: Uuid, ctx: &AppContext) -> Result<Option<db::Media>> {
        let result = match ctx.store.get::<MusicSearchResult>(id.to_string()) {
            Some(r) if r.media.kind == db::MediaKind::Artist => r,
            _ => return Ok(None),
        };
        ctx.store.delete(id.to_string());

        let artist_deezer_id = result
            .media
            .media_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Artist has no Deezer ID"))?;

        let t = std::time::Instant::now();
        tracing::info!(artist_id = %artist_deezer_id, "Fetching full artist discography for artist persist");
        let (artist, albums_with_tracks) =
            fetch_full_discography(&self.client, &artist_deezer_id).await?;
        let target_id = artist.id;
        tracing::info!(
            artist_id = %artist_deezer_id,
            elapsed_ms = t.elapsed().as_millis(),
            "Discography fetch done, saving to DB"
        );

        save_discography(artist, albums_with_tracks, ctx).await?;

        let media = db::Media::get_by_id(&ctx.db, &target_id).await?;
        Ok(media)
    }
}
