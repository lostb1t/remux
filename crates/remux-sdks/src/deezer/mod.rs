use std::{fmt, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{Endpoint, NoAuth, RestClient};

// ---------------------------------------------------------------------------
// Error / result wrapper
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeezerError {
    pub code: u32,
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

impl fmt::Display for DeezerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Deezer error {}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeezerResult<T> {
    Ok(T),
    Err { error: DeezerError },
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeezerList<T> {
    pub data: Vec<T>,
    pub total: Option<u64>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genre {
    pub id: u64,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Album (GET /album/{id}) — includes inline tracks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Album {
    pub id: u64,
    pub title: String,
    pub cover_xl: Option<String>,
    pub release_date: Option<String>,
    pub label: Option<String>,
    pub nb_tracks: Option<u32>,
    pub genres: Option<DeezerList<Genre>>,
    pub artist: Option<ArtistRef>,
    #[serde(default)]
    pub tracks: DeezerList<AlbumTrack>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumTrack {
    pub id: u64,
    pub title: String,
    pub duration: Option<u64>,
    pub track_position: Option<i64>,
    pub disk_number: Option<i64>,
    pub artist: ArtistRef,
}

// ---------------------------------------------------------------------------
// Artist (GET /artist/{id})
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Artist {
    pub id: u64,
    pub name: String,
    pub picture_xl: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtistAlbumRef {
    pub id: u64,
    pub title: Option<String>,
    pub cover_medium: Option<String>,
}

// ---------------------------------------------------------------------------
// Track (GET /track/{id}) — fallback when deezer_album is unknown
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub duration: Option<u64>,
    pub release_date: Option<String>,
    pub artist: ArtistRef,
    pub album: TrackAlbumRef,
    pub track_position: Option<i64>,
    pub disk_number: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackAlbumRef {
    pub id: u64,
    pub cover_xl: Option<String>,
    pub release_date: Option<String>,
}

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTrack {
    pub id: u64,
    pub title: String,
    pub duration: Option<u64>,
    pub artist: ArtistRef,
    pub album: SearchAlbumRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchAlbumRef {
    pub id: u64,
    pub title: String,
    pub cover_medium: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchAlbum {
    pub id: u64,
    pub title: String,
    pub cover_medium: Option<String>,
    pub artist: ArtistRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchArtist {
    pub id: u64,
    pub name: String,
    pub picture_xl: Option<String>,
}

// ---------------------------------------------------------------------------
// Playlist (GET /playlist/{id})
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub tracks: DeezerList<PlaylistTrack>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub id: u64,
    pub title: String,
    pub duration: u64,
    pub artist: ArtistRef,
    pub album: PlaylistTrackAlbum,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaylistTrackAlbum {
    pub id: u64,
    pub cover_xl: Option<String>,
    pub release_date: Option<String>,
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AlbumEndpoint {
    pub id: u64,
}

impl Endpoint for AlbumEndpoint {
    type Output = DeezerResult<Album>;
    fn path(&self) -> String {
        format!("album/{}", self.id)
    }
}

#[derive(Clone)]
pub struct ArtistEndpoint {
    pub id: u64,
}

impl Endpoint for ArtistEndpoint {
    type Output = DeezerResult<Artist>;
    fn path(&self) -> String {
        format!("artist/{}", self.id)
    }
}

#[derive(Clone, Serialize)]
pub struct ArtistAlbumsEndpoint {
    #[serde(skip)]
    pub id: u64,
    pub limit: u32,
}

impl Endpoint for ArtistAlbumsEndpoint {
    type Output = DeezerResult<DeezerList<ArtistAlbumRef>>;
    fn path(&self) -> String {
        format!("artist/{}/albums", self.id)
    }
    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Clone)]
pub struct TrackEndpoint {
    pub id: u64,
}

impl Endpoint for TrackEndpoint {
    type Output = DeezerResult<Track>;
    fn path(&self) -> String {
        format!("track/{}", self.id)
    }
}

#[derive(Clone, Serialize)]
pub struct SearchTracksEndpoint {
    pub q: String,
    pub limit: u32,
}

impl Endpoint for SearchTracksEndpoint {
    type Output = DeezerResult<DeezerList<SearchTrack>>;
    fn path(&self) -> String {
        "search".to_string()
    }
    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Clone, Serialize)]
pub struct SearchAlbumsEndpoint {
    pub q: String,
    pub limit: u32,
}

impl Endpoint for SearchAlbumsEndpoint {
    type Output = DeezerResult<DeezerList<SearchAlbum>>;
    fn path(&self) -> String {
        "search/album".to_string()
    }
    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Clone, Serialize)]
pub struct SearchArtistsEndpoint {
    pub q: String,
    pub limit: u32,
}

impl Endpoint for SearchArtistsEndpoint {
    type Output = DeezerResult<DeezerList<SearchArtist>>;
    fn path(&self) -> String {
        "search/artist".to_string()
    }
    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Clone)]
pub struct PlaylistEndpoint {
    pub id: String,
}

impl Endpoint for PlaylistEndpoint {
    type Output = DeezerResult<Playlist>;
    fn path(&self) -> String {
        format!("playlist/{}", self.id)
    }
}

// ---------------------------------------------------------------------------
// Client factory
// ---------------------------------------------------------------------------

pub fn client() -> RestClient<NoAuth> {
    RestClient::new("https://api.deezer.com/").expect("Deezer base URL is valid")
}
