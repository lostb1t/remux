//! Eclipse music addon protocol.
//!
//! An Eclipse addon is a plain HTTP server that serves music: it answers
//! `GET /manifest.json` describing itself, `GET /search?q=` with results, and
//! `GET /stream/{id}` with a playable URL. Everything else is optional.
//!
//! Deliberately kept separate from [`crate::stremio`] rather than folded into
//! it: the two protocols share only the word "addon". Stremio addresses items
//! by a globally meaningful id (`tt0111161`) under `/{resource}/{type}/{id}.json`
//! and returns `{"metas": [...]}`; Eclipse addresses them by an id that means
//! nothing outside the addon that minted it, under flat `/{resource}/{id}`
//! routes, and returns type-specific arrays. Modelling them as one type would
//! mean a manifest whose fields are half-inapplicable whichever kind it is.
//!
//! Reference: <https://eclipsemusic.app/docs>

use crate::{Endpoint, RestClient};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// Eclipse ids are opaque addon-minted strings, so an id containing `/`, `?`,
/// or `#` would otherwise silently change which route is requested.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'%');

/// What an addon declares it can do, in `manifest.resources`.
///
/// Unknown values are preserved in [`Resource::Other`] rather than rejected: an
/// addon that ships a capability newer than this server must still install and
/// serve the capabilities we do understand.
#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Resource {
    /// `GET /search?q=` — required.
    Search,
    /// `GET /stream/{id}` — required.
    Stream,
    /// `/album/{id}`, `/artist/{id}`, `/playlist/{id}` detail endpoints, and
    /// `/catalog/{id}` rows declared in `manifest.catalogs`. The Eclipse docs
    /// use this one value for both.
    Catalog,
    /// `GET /resolve-isrc?isrc=` — maps an ISRC to one of the addon's own ids.
    Isrc,
    /// `GET /resolve?isrc=&title=&artist=&durationMs=` — the addon names its
    /// own item for a recording described by identity rather than by id.
    Resolve,
    /// The addon declares a `settings` schema whose values are echoed back as
    /// query parameters on every request.
    Settings,
    #[strum(to_string = "{0}")]
    #[serde(untagged)]
    Other(String),
}

/// Content types an addon serves, in `manifest.types` and on catalog items.
#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ItemType {
    Track,
    Album,
    Artist,
    Playlist,
    /// Generic audio file.
    File,
}

/// Player UI mode the addon's content wants. Eclipse never infers this from
/// track metadata — the manifest field is the only signal.
#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ContentType {
    #[default]
    Music,
    Audiobook,
    Podcast,
}

/// `GET /manifest.json`
#[derive(Debug, Clone)]
pub struct ManifestEndpoint;

impl Endpoint for ManifestEndpoint {
    type Output = Manifest;

    fn path(&self) -> String {
        "/manifest.json".into()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub resources: Vec<Resource>,
    /// Optional per the docs; an addon that omits it still serves tracks.
    #[serde(default, deserialize_with = "deserialize_lossy_seq")]
    pub types: Vec<ItemType>,
    #[serde(default)]
    pub content_type: ContentType,
    /// Rows for Home/Discover/Browse, served from `GET /catalog/{id}`.
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
}

impl Manifest {
    pub fn has(&self, resource: &Resource) -> bool {
        self.resources
            .contains(resource)
    }

    /// Whether the addon serves `types`, treating an absent/empty `types` list
    /// as "tracks", which is what a search-only addon in practice returns.
    pub fn serves(&self, item_type: ItemType) -> bool {
        if self
            .types
            .is_empty()
        {
            return item_type == ItemType::Track;
        }
        self.types
            .contains(&item_type)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ItemType,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// A track as it appears in search results, catalog rows, and detail payloads.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    /// Required by the docs, but a podcast/audiobook addon in practice omits
    /// it; an absent artist is not a reason to drop an otherwise playable item.
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Seconds. Catalog rows use `durationMs` instead — see [`Self::seconds`].
    pub duration: Option<i64>,
    pub duration_ms: Option<i64>,
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_isrc")]
    pub isrc: Option<String>,
    pub format: Option<String>,
    /// Present when the addon can hand out a URL without a `/stream` round
    /// trip. Callers must still honour `/stream` for expiring URLs.
    pub stream_url: Option<String>,
    pub explicit: Option<bool>,
    pub hi_res: Option<bool>,
}

impl Track {
    /// Runtime in whole seconds, from whichever duration field the addon sent.
    pub fn seconds(&self) -> Option<i64> {
        self.duration
            .or_else(|| {
                self.duration_ms
                    .map(|ms| ms / 1000)
            })
            .filter(|s| *s > 0)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub artwork_url: Option<String>,
    pub track_count: Option<i64>,
    /// The docs accept both `"2024"` and `2024`.
    #[serde(default, deserialize_with = "deserialize_opt_year")]
    pub year: Option<i64>,
    pub description: Option<String>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub artwork_url: Option<String>,
    pub bio: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub top_tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub creator: Option<String>,
    pub track_count: Option<i64>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// `GET /search?q={query}`
///
/// Every array is optional; an addon returns only what it has.
#[derive(Debug, Clone, Serialize)]
pub struct SearchEndpoint {
    pub q: String,
}

impl Endpoint for SearchEndpoint {
    type Output = SearchResponse;

    fn path(&self) -> String {
        "/search".into()
    }

    fn query_params(&self) -> impl Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

/// `GET /stream/{id}`
#[derive(Debug, Clone)]
pub struct StreamEndpoint {
    pub id: String,
}

impl Endpoint for StreamEndpoint {
    type Output = Stream;

    fn path(&self) -> String {
        format!("/stream/{}", utf8_percent_encode(&self.id, PATH_SEGMENT))
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub url: String,
    pub format: Option<String>,
    pub quality: Option<String>,
    /// Unix timestamp after which `url` stops working.
    pub expires_at: Option<i64>,
    pub codec: Option<String>,
    pub container: Option<String>,
    /// `none` when `url` is the audio file itself, `hls` for an .m3u8
    /// playlist, `dash` for an .mpd.
    pub manifest: Option<String>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub title: String,
    /// Offset from the start of the track, in seconds.
    pub start_time: f64,
}

/// `GET /album/{id}`
#[derive(Debug, Clone)]
pub struct AlbumEndpoint {
    pub id: String,
}

impl Endpoint for AlbumEndpoint {
    type Output = Album;

    fn path(&self) -> String {
        format!("/album/{}", utf8_percent_encode(&self.id, PATH_SEGMENT))
    }
}

/// `GET /artist/{id}`
#[derive(Debug, Clone)]
pub struct ArtistEndpoint {
    pub id: String,
}

impl Endpoint for ArtistEndpoint {
    type Output = Artist;

    fn path(&self) -> String {
        format!("/artist/{}", utf8_percent_encode(&self.id, PATH_SEGMENT))
    }
}

/// `GET /playlist/{id}`
#[derive(Debug, Clone)]
pub struct PlaylistEndpoint {
    pub id: String,
}

impl Endpoint for PlaylistEndpoint {
    type Output = Playlist;

    fn path(&self) -> String {
        format!("/playlist/{}", utf8_percent_encode(&self.id, PATH_SEGMENT))
    }
}

/// `GET /catalog/{id}?skip={n}`
///
/// `skip` is a multiple of 100; a page shorter than 100 items is the end of the
/// row.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEndpoint {
    #[serde(skip)]
    pub id: String,
    pub skip: Option<u32>,
}

impl Endpoint for CatalogEndpoint {
    type Output = CatalogResponse;

    fn path(&self) -> String {
        format!("/catalog/{}", utf8_percent_encode(&self.id, PATH_SEGMENT))
    }

    fn query_params(&self) -> impl Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogResponse {
    #[serde(default)]
    pub items: Vec<CatalogItem>,
}

/// One row entry. `kind` says which of the detail endpoints opens it.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ItemType,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[serde(default, deserialize_with = "deserialize_isrc")]
    pub isrc: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub explicit: Option<bool>,
    pub hi_res: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_opt_year")]
    pub year: Option<i64>,
}

/// `GET /resolve-isrc?isrc={isrc}`
///
/// A 404 or a null id is the addon saying "I don't have that recording" — a
/// normal answer, not an error.
#[derive(Debug, Clone, Serialize)]
pub struct ResolveIsrcEndpoint {
    pub isrc: String,
}

impl Endpoint for ResolveIsrcEndpoint {
    type Output = Option<ResolveIsrcResponse>;

    fn path(&self) -> String {
        "/resolve-isrc".into()
    }

    fn query_params(&self) -> impl Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveIsrcResponse {
    /// The docs accept `trackId` and `id` interchangeably.
    #[serde(default, alias = "id")]
    pub track_id: Option<String>,
}

/// `GET /resolve?isrc=&title=&artist=&durationMs=`
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveEndpoint {
    pub isrc: Option<String>,
    pub title: String,
    pub artist: String,
    pub duration_ms: Option<i64>,
}

impl Endpoint for ResolveEndpoint {
    type Output = Option<ResolveResponse>;

    fn path(&self) -> String {
        "/resolve".into()
    }

    fn query_params(&self) -> impl Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub item: Option<CatalogItem>,
}

// ---------------------------------------------------------------------------
// Deserializers
// ---------------------------------------------------------------------------

/// Normalizes an ISRC to bare uppercase and rejects anything that isn't one.
///
/// The docs promise dashes and lower case are accepted, and warn that an empty
/// string is not an ISRC. Anything malformed becomes `None` here rather than at
/// each use site: an id that only looks like a recording code is worse than no
/// id at all, because it silently matches the wrong recording.
fn deserialize_isrc<'de, D>(de: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(de)?;
    Ok(raw.and_then(|s| normalize_isrc(&s)))
}

/// `Some(normalized)` for a well-formed ISRC, `None` otherwise.
///
/// Shape is 2 alphanumeric country characters, 3 alphanumeric registrant
/// characters, then 7 digits (2 year + 5 designation).
pub fn normalize_isrc(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    if cleaned.len() != 12 {
        return None;
    }
    let bytes = cleaned.as_bytes();
    let alnum = bytes[..5]
        .iter()
        .all(|b| b.is_ascii_alphanumeric());
    let digits = bytes[5..]
        .iter()
        .all(|b| b.is_ascii_digit());
    (alnum && digits).then_some(cleaned)
}

/// Accepts `"2024"` or `2024`; a bare year string that isn't a number, or a
/// full date, yields `None`.
fn deserialize_opt_year<'de, D>(de: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(de)? {
        Some(serde_json::Value::Number(n)) => Ok(n.as_i64()),
        Some(serde_json::Value::String(s)) => Ok(s
            .trim()
            .get(..4)
            .and_then(|y| {
                y.parse()
                    .ok()
            })),
        _ => Ok(None),
    }
}

/// Drops entries that don't parse instead of failing the whole list, so one
/// unrecognized `types` value cannot make an addon uninstallable.
fn deserialize_lossy_seq<'de, D, T>(de: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(de)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?
        .with_retry(crate::ExponentialBackoff::builder().build_with_max_retries(3)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_keeps_unknown_resources_and_types() {
        let m: Manifest = serde_json::from_value(serde_json::json!({
            "id": "com.example.a",
            "name": "A",
            "version": "1.0.0",
            "resources": ["search", "stream", "isrc", "telepathy"],
            "types": ["track", "album", "hologram"],
        }))
        .unwrap();

        assert!(m.has(&Resource::Search));
        assert!(m.has(&Resource::Isrc));
        assert!(m.has(&Resource::Other("telepathy".into())));
        assert!(!m.has(&Resource::Catalog));
        // An unrecognized type is dropped rather than failing the manifest.
        assert_eq!(m.types, vec![ItemType::Track, ItemType::Album]);
        assert_eq!(m.content_type, ContentType::Music);
    }

    /// An addon that declares no types still serves tracks — otherwise a
    /// search-only addon would be treated as serving nothing.
    #[test]
    fn manifest_without_types_serves_tracks_only() {
        let m: Manifest = serde_json::from_value(serde_json::json!({
            "id": "com.example.a", "name": "A", "version": "1.0.0",
            "resources": ["search", "stream"],
        }))
        .unwrap();

        assert!(m.serves(ItemType::Track));
        assert!(!m.serves(ItemType::Album));
    }

    #[test]
    fn track_duration_comes_from_either_field() {
        let secs = Track {
            duration: Some(240),
            ..Default::default()
        };
        let millis = Track {
            duration_ms: Some(181_000),
            ..Default::default()
        };
        assert_eq!(secs.seconds(), Some(240));
        assert_eq!(millis.seconds(), Some(181));
        // A zero duration is no duration; it would otherwise render as "0:00".
        assert_eq!(
            Track {
                duration: Some(0),
                ..Default::default()
            }
            .seconds(),
            None
        );
        assert_eq!(Track::default().seconds(), None);
    }

    #[test]
    fn isrc_is_normalized_or_dropped() {
        assert_eq!(
            normalize_isrc("us-rc1-19-03813"),
            Some("USRC11903813".to_string())
        );
        assert_eq!(normalize_isrc("USRC11903813"), Some("USRC11903813".into()));
        // The docs are explicit that an empty string is not an ISRC.
        assert_eq!(normalize_isrc(""), None);
        assert_eq!(normalize_isrc("USRC1190381"), None, "too short");
        assert_eq!(normalize_isrc("USRC119038134"), None, "too long");
        assert_eq!(
            normalize_isrc("USRC1190381X"),
            None,
            "designation must be digits"
        );
    }

    /// A malformed ISRC must not survive deserialization: downstream treats an
    /// ISRC as an exact recording identity, so a plausible-looking wrong value
    /// plays the wrong song.
    #[test]
    fn track_drops_malformed_isrc() {
        let t: Track = serde_json::from_value(serde_json::json!({
            "id": "t1", "title": "T", "artist": "A", "isrc": ""
        }))
        .unwrap();
        assert_eq!(t.isrc, None);

        let t: Track = serde_json::from_value(serde_json::json!({
            "id": "t1", "title": "T", "artist": "A", "isrc": "us-rc1-19-03813"
        }))
        .unwrap();
        assert_eq!(
            t.isrc
                .as_deref(),
            Some("USRC11903813")
        );
    }

    #[test]
    fn album_year_accepts_string_or_number() {
        let from_str: Album = serde_json::from_value(
            serde_json::json!({"id": "a", "title": "T", "year": "2024"}),
        )
        .unwrap();
        let from_num: Album = serde_json::from_value(
            serde_json::json!({"id": "a", "title": "T", "year": 2024}),
        )
        .unwrap();
        assert_eq!(from_str.year, Some(2024));
        assert_eq!(from_num.year, Some(2024));
    }

    #[test]
    fn search_response_tolerates_missing_arrays() {
        let r: SearchResponse = serde_json::from_value(serde_json::json!({
            "tracks": [{"id": "t1", "title": "Song", "artist": "Artist"}]
        }))
        .unwrap();
        assert_eq!(
            r.tracks
                .len(),
            1
        );
        assert!(
            r.albums
                .is_empty()
        );
        assert!(
            r.artists
                .is_empty()
        );
    }

    /// An opaque addon id may contain characters that would otherwise change
    /// which route is requested.
    #[test]
    fn endpoint_paths_encode_the_id() {
        assert_eq!(
            StreamEndpoint { id: "a/b?c".into() }.path(),
            "/stream/a%2Fb%3Fc"
        );
        assert_eq!(AlbumEndpoint { id: "al 1".into() }.path(), "/album/al%201");
    }

    #[test]
    fn catalog_endpoint_pages_with_skip() {
        let first = CatalogEndpoint {
            id: "top".into(),
            skip: None,
        };
        let second = CatalogEndpoint {
            id: "top".into(),
            skip: Some(100),
        };
        assert_eq!(first.path(), "/catalog/top");
        assert!(
            first
                .query()
                .is_empty()
        );
        assert_eq!(
            second.query(),
            vec![("skip".to_string(), "100".to_string())]
        );
    }

    #[test]
    fn resolve_isrc_accepts_either_id_field() {
        let with_track_id: ResolveIsrcResponse =
            serde_json::from_value(serde_json::json!({"trackId": "t1"})).unwrap();
        let with_id: ResolveIsrcResponse =
            serde_json::from_value(serde_json::json!({"id": "t1"})).unwrap();
        let empty: ResolveIsrcResponse =
            serde_json::from_value(serde_json::json!({"trackId": null})).unwrap();

        assert_eq!(
            with_track_id
                .track_id
                .as_deref(),
            Some("t1")
        );
        assert_eq!(
            with_id
                .track_id
                .as_deref(),
            Some("t1")
        );
        assert_eq!(empty.track_id, None);
    }

    #[test]
    fn stream_carries_chapters_and_routing_hints() {
        let s: Stream = serde_json::from_value(serde_json::json!({
            "url": "https://cdn.example.com/a.flac",
            "codec": "flac",
            "container": "flac",
            "manifest": "none",
            "sampleRate": 44100,
            "bitDepth": 16,
            "chapters": [{"title": "One", "startTime": 0}, {"title": "Two", "startTime": 1823}]
        }))
        .unwrap();

        assert_eq!(s.sample_rate, Some(44100));
        assert_eq!(
            s.codec
                .as_deref(),
            Some("flac")
        );
        assert_eq!(
            s.chapters
                .len(),
            2
        );
        assert_eq!(s.chapters[1].start_time, 1823.0);
    }
}
