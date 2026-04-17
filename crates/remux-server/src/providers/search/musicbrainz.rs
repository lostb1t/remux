use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

use super::SearchService;

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0 (https://github.com/remux)")
        .build()
        .expect("failed to build HTTP client")
}


#[derive(Deserialize)]
struct ArtistCredit {
    name: Option<String>,
}

fn first_artist(credits: &[ArtistCredit]) -> Option<String> {
    credits.first().and_then(|a| a.name.clone())
}

fn year_to_naive(date_str: Option<&str>) -> Option<chrono::NaiveDateTime> {
    date_str
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<i32>().ok())
        .and_then(|y| chrono::NaiveDate::from_ymd_opt(y, 1, 1))
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
}


/// Search backend backed by MusicBrainz — handles albums (release groups).
///
/// Pure HTTP, no subprocess. Typically responds in 50–200 ms.
pub struct MusicBrainzAlbumSearchService {
    client: reqwest::Client,
}

impl Default for MusicBrainzAlbumSearchService {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[derive(Deserialize)]
struct ReleaseGroupSearch {
    #[serde(rename = "release-groups")]
    release_groups: Vec<ReleaseGroup>,
}

#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
}

#[async_trait]
impl SearchService for MusicBrainzAlbumSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Album]
    }

    async fn search(&self, _kind: &db::MediaKind, query: &str, limit: usize, _ctx: &AppContext) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();

        let url = format!(
            "https://musicbrainz.org/ws/2/release-group?query={}&type=album&limit={}&fmt=json",
            urlencoding::encode(query),
            limit.min(25)
        );

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            tracing::warn!(query, status = %resp.status(), "MusicBrainz album search error");
            return Ok(vec![]);
        }

        let data: ReleaseGroupSearch = resp.json().await?;

        let results = data
            .release_groups
            .into_iter()
            .map(|rg| {
                let artist = first_artist(&rg.artist_credit);
                let poster = Some(format!(
                    "https://coverartarchive.org/release-group/{}/front-250",
                    rg.id
                ));
                db::Media {
                    id: crate::utils::get_stable_uuid(format!("mb-album:{}", rg.id)),
                    title: rg.title,
                    kind: db::MediaKind::Album,
                    media_id: Some(rg.id),
                    poster,
                    released_at: year_to_naive(rg.first_release_date.as_deref()),
                    description: artist.map(|a| format!("by {}", a)),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();

        tracing::info!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "MusicBrainz album search done"
        );

        Ok(results)
    }
}


/// Search backend backed by MusicBrainz — handles tracks (recordings).
///
/// Pure HTTP, no subprocess. Typically responds in 50–200 ms.
/// Note: results have no stream URL; the stream service resolves that on demand.
pub struct MusicBrainzTrackSearchService {
    client: reqwest::Client,
}

impl Default for MusicBrainzTrackSearchService {
    fn default() -> Self {
        Self { client: build_client() }
    }
}

#[derive(Deserialize)]
struct RecordingSearch {
    recordings: Vec<Recording>,
}

#[derive(Deserialize)]
struct Recording {
    id: String,
    title: String,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<ArtistCredit>,
    /// Duration in milliseconds.
    length: Option<u64>,
    #[serde(default)]
    releases: Vec<RecordingRelease>,
}

#[derive(Deserialize)]
struct RecordingRelease {
    #[serde(rename = "release-group")]
    release_group: Option<RecordingReleaseGroup>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct RecordingReleaseGroup {
    id: String,
}

#[async_trait]
impl SearchService for MusicBrainzTrackSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn search(&self, _kind: &db::MediaKind, query: &str, limit: usize, _ctx: &AppContext) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();

        let url = format!(
            "https://musicbrainz.org/ws/2/recording?query={}&limit={}&fmt=json",
            urlencoding::encode(query),
            limit.min(25)
        );

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            tracing::warn!(query, status = %resp.status(), "MusicBrainz track search error");
            return Ok(vec![]);
        }

        let data: RecordingSearch = resp.json().await?;

        let results = data
            .recordings
            .into_iter()
            .map(|rec| {
                let artist = first_artist(&rec.artist_credit);
                // Use the first release's release-group ID for cover art.
                let poster = rec
                    .releases
                    .first()
                    .and_then(|r| r.release_group.as_ref())
                    .map(|rg| format!(
                        "https://coverartarchive.org/release-group/{}/front-250",
                        rg.id
                    ));
                let released_at = rec
                    .releases
                    .first()
                    .and_then(|r| year_to_naive(r.date.as_deref()));
                db::Media {
                    id: crate::utils::get_stable_uuid(format!("mb-track:{}", rec.id)),
                    title: rec.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(rec.id),
                    poster,
                    released_at,
                    // length is in ms; runtime is in seconds
                    runtime: rec.length.map(|ms| (ms / 1000) as i64),
                    description: artist.map(|a| format!("by {}", a)),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();

        tracing::info!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "MusicBrainz track search done"
        );

        Ok(results)
    }
}
