use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::NaiveDateTime;
use futures::Stream;
use futures::stream::{self, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, CatalogInfo, MediaKind, ResourceType,
};
use crate::sdks::deezer as dz;
use crate::sdks::{self, CachedEndpoint};
use crate::{AppContext, common, db};

const CACHE_TTL: Duration = Duration::from_secs(60);

fn parse_release_date(s: &str) -> Option<NaiveDateTime> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
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
                MediaKind::Track,
                MediaKind::Album,
                MediaKind::Artist,
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

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
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
            client: dz::client(),
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
    client: sdks::RestClient<sdks::NoAuth>,
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
    ) -> Result<Option<db::Media>> {
        // Prefer the cached album endpoint — avoids per-track quota hits.
        if let Some(album_id) = base.external_ids.deezer_album.map(|id| id as u64) {
            match self
                .client
                .execute(dz::AlbumEndpoint { id: album_id }.with_cache(CACHE_TTL))
                .await
            {
                Ok(dz::DeezerResult::Ok(album)) => {
                    if let Some(track) = album
                        .tracks
                        .data
                        .iter()
                        .find(|t| t.id.to_string() == deezer_id)
                    {
                        let released_at =
                            album.release_date.as_deref().and_then(parse_release_date);
                        let track_artist = if track.artist.name.is_empty() {
                            album
                                .artist
                                .as_ref()
                                .map(|a| a.name.clone())
                                .unwrap_or_default()
                        } else {
                            track.artist.name.clone()
                        };
                        let mut patch = db::Media {
                            id: base.id,
                            title: track.title.clone(),
                            kind: db::MediaKind::Track,
                            media_id: Some(track.id.to_string()),
                            runtime: track.duration.map(|s| s as i64),
                            released_at,
                            description: Some(format!("by {}", track_artist)),
                            ..Default::default()
                        };
                        if let Some(url) = album.cover_xl {
                            patch.set_image(db::ImageKind::Primary, url);
                        }
                        return Ok(Some(patch));
                    }
                    tracing::debug!(
                        deezer_id,
                        album_id,
                        "track not found in cached album, falling back to track endpoint"
                    );
                }
                Ok(dz::DeezerResult::Err { error }) => {
                    warn!(deezer_id, %error, "Deezer album endpoint error in track fetch, falling back");
                }
                Err(e) => {
                    warn!(deezer_id, error = %e, "Deezer album endpoint failed in track fetch, falling back");
                }
            }
        }

        // Fallback: individual track endpoint (for rows without deezer_album).
        let Ok(id) = deezer_id.parse::<u64>() else {
            return Ok(None);
        };
        match self.client.execute(dz::TrackEndpoint { id }).await {
            Ok(dz::DeezerResult::Ok(t)) => {
                let released_at = t
                    .release_date
                    .as_deref()
                    .or(t.album.release_date.as_deref())
                    .and_then(parse_release_date);
                tracing::debug!(deezer_id, "Deezer track detail fetched");
                let mut patch = db::Media {
                    id: base.id,
                    title: t.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(t.id.to_string()),
                    runtime: t.duration.map(|s| s as i64),
                    released_at,
                    description: Some(format!("by {}", t.artist.name)),
                    ..Default::default()
                };
                if let Some(url) = t.album.cover_xl {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                Ok(Some(patch))
            }
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(deezer_id, %error, "Deezer track detail returned error");
                Ok(None)
            }
            Err(e) => {
                warn!(deezer_id, error = %e, "Deezer track detail HTTP error");
                Ok(None)
            }
        }
    }

    async fn fetch_album_meta(
        &self,
        deezer_id: &str,
        base: &db::Media,
    ) -> Result<Option<db::Media>> {
        let Ok(id) = deezer_id.parse::<u64>() else {
            return Ok(None);
        };
        let a = match self
            .client
            .execute(dz::AlbumEndpoint { id }.with_cache(CACHE_TTL))
            .await
        {
            Ok(dz::DeezerResult::Ok(a)) => a,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(deezer_id, %error, "Deezer album detail returned error");
                return Ok(None);
            }
            Err(e) => {
                warn!(deezer_id, error = %e, "Deezer album detail HTTP error");
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
        tracing::debug!(deezer_id, nb_tracks = ?a.nb_tracks, "Deezer album detail fetched");
        let mut patch = db::Media {
            id: base.id,
            title: a.title,
            kind: db::MediaKind::Album,
            media_id: Some(a.id.to_string()),
            released_at,
            description: Some(desc_parts.join(" · ")),
            ..Default::default()
        };
        if let Some(url) = a.cover_xl {
            patch.set_image(db::ImageKind::Primary, url);
        }
        Ok(Some(patch))
    }

    // --- Hierarchy ---

    async fn fetch_full_album_detail(&self, album_id: u64) -> Option<dz::Album> {
        match self
            .client
            .execute(dz::AlbumEndpoint { id: album_id }.with_cache(CACHE_TTL))
            .await
        {
            Ok(dz::DeezerResult::Ok(album)) => Some(album),
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(album_id, %error, "Deezer album detail returned error, skipping");
                None
            }
            Err(e) => {
                warn!(album_id, error = %e, "Deezer album detail HTTP error, skipping");
                None
            }
        }
    }

    fn build_album_children(
        detail: dz::Album,
        album_id: Uuid,
        album_title: String,
        artist_id: Option<Uuid>,
        artist_title: String,
    ) -> Vec<db::Media> {
        let deezer_album_id = detail.id;
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
                let mut t = db::Media {
                    id: common::get_stable_uuid(format!("track:{}", track.id)),
                    title: track.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(track.id.to_string()),
                    runtime: track.duration.map(|s| s as i64),
                    released_at,
                    description: Some(format!("by {}", track_artist)),
                    idx: track.track_position,
                    parent_idx: track.disk_number,
                    parent_id: Some(album_id),
                    grandparent_id: artist_id,
                    parent_title: Some(album_title.clone()),
                    series_title: Some(artist_title.clone()),
                    external_ids: db::ExternalIds {
                        deezer_album: Some(deezer_album_id as i64),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                if let Some(url) = detail.cover_xl.clone() {
                    t.set_image(db::ImageKind::Primary, url);
                }
                t
            })
            .collect()
    }

    /// Returns minimal Album stubs for an Artist — direct children only, no track fetching.
    async fn list_artist_albums(&self, root: &db::Media) -> Result<Vec<db::Media>> {
        let Some(artist_id) = root.media_id.as_deref() else {
            return Ok(vec![]);
        };
        let Ok(artist_id_num) = artist_id.parse::<u64>() else {
            return Ok(vec![]);
        };

        let albums = match self
            .client
            .execute(
                dz::ArtistAlbumsEndpoint {
                    id: artist_id_num,
                    limit: 1000,
                }
                .with_cache(CACHE_TTL),
            )
            .await
        {
            Ok(dz::DeezerResult::Ok(list)) => list.data,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(artist_id, %error, "Deezer artist albums returned error");
                return Ok(vec![]);
            }
            Err(e) => {
                warn!(artist_id, error = %e, "Deezer artist albums HTTP error");
                return Ok(vec![]);
            }
        };

        let deezer_artist_id = artist_id.parse::<i64>().ok();
        Ok(albums
            .into_iter()
            .map(|album| {
                let mut m = db::Media {
                    id: common::get_stable_uuid(format!("album:{}", album.id)),
                    title: album.title.unwrap_or_default(),
                    kind: db::MediaKind::Album,
                    media_id: Some(album.id.to_string()),
                    parent_id: Some(root.id),
                    grandparent_id: Some(root.id),
                    series_title: Some(root.title.clone()),
                    external_ids: db::ExternalIds {
                        deezer_artist: deezer_artist_id,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                if let Some(url) = album.cover_medium {
                    m.set_image(db::ImageKind::Primary, url);
                }
                m
            })
            .collect())
    }

    async fn sync_artist_children(&self, root: &db::Media) -> Result<Vec<db::Media>> {
        let Some(artist_id) = root.media_id.as_deref() else {
            return Ok(vec![]);
        };
        let Ok(artist_id_num) = artist_id.parse::<u64>() else {
            return Ok(vec![]);
        };

        let artist = match self
            .client
            .execute(dz::ArtistEndpoint { id: artist_id_num }.with_cache(CACHE_TTL))
            .await
        {
            Ok(dz::DeezerResult::Ok(a)) => a,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(artist_id, %error, "Deezer artist detail returned error");
                return Ok(vec![]);
            }
            Err(e) => {
                warn!(artist_id, error = %e, "Deezer artist detail HTTP error");
                return Ok(vec![]);
            }
        };

        let artist_title = if root.title.is_empty() {
            artist.name
        } else {
            root.title.clone()
        };
        let artist_poster = root
            .get_image(db::ImageKind::Primary)
            .map(str::to_owned)
            .or(artist.picture_xl);

        let albums = match self
            .client
            .execute(
                dz::ArtistAlbumsEndpoint {
                    id: artist_id_num,
                    limit: 1000,
                }
                .with_cache(CACHE_TTL),
            )
            .await
        {
            Ok(dz::DeezerResult::Ok(list)) => list.data,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(artist_id, %error, "Deezer artist albums returned error");
                vec![]
            }
            Err(e) => {
                warn!(artist_id, error = %e, "Deezer artist albums HTTP error");
                vec![]
            }
        };

        let album_futs = albums.into_iter().map(|album| {
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

                let mut album_media = db::Media {
                    id: common::get_stable_uuid(format!("album:{}", detail.id)),
                    title: detail.title.clone(),
                    kind: db::MediaKind::Album,
                    media_id: Some(detail.id.to_string()),
                    released_at,
                    description: Some(desc_parts.join(" · ")),
                    parent_id: Some(root_id),
                    grandparent_id: Some(root_id),
                    series_title: Some(artist_title.clone()),
                    ..Default::default()
                };
                if let Some(url) = detail.cover_xl.clone().or(artist_poster.clone()) {
                    album_media.set_image(db::ImageKind::Primary, url);
                }

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
            root.grandparent_id.or(root.parent_id),
            artist_title,
        ))
    }

    // --- Search ---

    async fn search_tracks(
        &self,
        query: &str,
        limit: usize,
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer track search starting");

        let data = match self
            .client
            .execute(dz::SearchTracksEndpoint {
                q: query.to_string(),
                limit: limit.min(50) as u32,
            })
            .await
        {
            Ok(dz::DeezerResult::Ok(list)) => list.data,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(query, %error, "Deezer track search returned error");
                return Ok(vec![]);
            }
            Err(e) => {
                warn!(query, error = %e, "Deezer track search HTTP error");
                return Ok(vec![]);
            }
        };

        let results: Vec<_> = data.into_iter().map(track_to_result).collect();
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
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer album search starting");

        let data = match self
            .client
            .execute(dz::SearchAlbumsEndpoint {
                q: query.to_string(),
                limit: limit.min(25) as u32,
            })
            .await
        {
            Ok(dz::DeezerResult::Ok(list)) => list.data,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(query, %error, "Deezer album search returned error");
                return Ok(vec![]);
            }
            Err(e) => {
                warn!(query, error = %e, "Deezer album search HTTP error");
                return Ok(vec![]);
            }
        };

        let results: Vec<_> = data.into_iter().map(album_to_result).collect();
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
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        tracing::debug!(query, limit, "Deezer artist search starting");

        let data = match self
            .client
            .execute(dz::SearchArtistsEndpoint {
                q: query.to_string(),
                limit: limit.min(25) as u32,
            })
            .await
        {
            Ok(dz::DeezerResult::Ok(list)) => list.data,
            Ok(dz::DeezerResult::Err { error }) => {
                warn!(query, %error, "Deezer artist search returned error");
                return Ok(vec![]);
            }
            Err(e) => {
                warn!(query, error = %e, "Deezer artist search HTTP error");
                return Ok(vec![]);
            }
        };

        let results: Vec<_> = data
            .into_iter()
            .map(|a| {
                let mut artist = db::Media {
                    id: common::get_stable_uuid(format!("artist:{}", a.id)),
                    title: a.name,
                    kind: db::MediaKind::Artist,
                    media_id: Some(a.id.to_string()),
                    ..Default::default()
                };
                if let Some(url) = a.picture_xl {
                    artist.set_image(db::ImageKind::Primary, url);
                }
                artist
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

    // --- Catalog (playlist) ---

    async fn fetch_playlist_stream(
        &self,
        _ctx: &AppContext,
        playlist_id: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>> {
        let playlist = match self
            .client
            .execute(dz::PlaylistEndpoint {
                id: playlist_id.to_string(),
            })
            .await
        {
            Ok(dz::DeezerResult::Ok(p)) => p,
            Ok(dz::DeezerResult::Err { error }) => {
                return Err(anyhow!("Deezer playlist error: {}", error));
            }
            Err(e) => {
                return Err(anyhow!(
                    "Deezer playlist {} HTTP error: {}",
                    playlist_id,
                    e
                ));
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
                let mut media = db::Media {
                    id: common::get_stable_uuid(format!("track:{}", track.id)),
                    title: track.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(track.id.to_string()),
                    runtime: Some(track.duration as i64),
                    released_at,
                    description: Some(format!("by {}", track.artist.name)),
                    ..Default::default()
                };
                if let Some(url) = track.album.cover_xl {
                    media.set_image(db::ImageKind::Primary, url);
                }
                media
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
            .map(|id| CatalogInfo::new(id.clone(), format!("Deezer playlist {id}")))
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
    ) -> Result<Option<db::Media>> {
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

    fn supports_children(&self, root: &db::Media) -> bool {
        matches!(root.kind, db::MediaKind::Artist | db::MediaKind::Album)
    }

    async fn get_children(
        &self,
        root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if !self.supports_children(root) {
            return Ok(None);
        }
        let children = match root.kind {
            db::MediaKind::Artist => self.list_artist_albums(root).await?,
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

fn track_to_result(t: dz::SearchTrack) -> db::Media {
    let album_id = common::get_stable_uuid(format!("album:{}", t.album.id));
    let mut track = db::Media {
        id: common::get_stable_uuid(format!("track:{}", t.id)),
        title: t.title,
        kind: db::MediaKind::Track,
        media_id: Some(t.id.to_string()),
        runtime: t.duration.map(|s| s as i64),
        description: Some(format!("by {}", t.artist.name)),
        parent_id: Some(album_id),
        parent_title: Some(t.album.title),
        series_title: Some(t.artist.name.clone()),
        external_ids: db::ExternalIds {
            deezer_artist: Some(t.artist.id as i64),
            deezer_album: Some(t.album.id as i64),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Some(url) = t.album.cover_medium {
        track.set_image(db::ImageKind::Primary, url);
    }
    track
}

fn album_to_result(a: dz::SearchAlbum) -> db::Media {
    let mut album = db::Media {
        id: common::get_stable_uuid(format!("album:{}", a.id)),
        title: a.title,
        kind: db::MediaKind::Album,
        media_id: Some(a.id.to_string()),
        description: Some(format!("by {}", a.artist.name)),
        series_title: Some(a.artist.name.clone()),
        external_ids: db::ExternalIds {
            deezer_artist: Some(a.artist.id as i64),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Some(url) = a.cover_medium {
        album.set_image(db::ImageKind::Primary, url);
    }
    album
}
