//! HTTP access to a single Eclipse music addon.
//!
//! Mirrors [`super::stremio::StremioService`]: one struct per addon instance,
//! endpoint TTLs chosen per resource, and query parameters carried in the
//! configured URL forwarded onto every request. Eclipse relies on that last
//! part more heavily than Stremio does — an addon's declared settings arrive
//! back as query parameters on *every* call, and token-authenticated addons put
//! the token in the URL path.

use crate::{
    sdks,
    sdks::{CachedEndpoint, ClientError, Endpoint, WithExtraQuery},
};
use anyhow::Result;
use futures::stream::{self, Stream, StreamExt};
use std::{pin::Pin, time::Duration};
use tracing::{debug, warn};

/// The manifest changes only when the operator redeploys the addon.
const MANIFEST_TTL: Duration = Duration::from_secs(3600);
/// Search is the user-facing latency path but its results are volatile.
const SEARCH_TTL: Duration = Duration::from_secs(60);
/// Detail payloads (album/artist/playlist track lists) are effectively static.
const DETAIL_TTL: Duration = Duration::from_secs(3600);
/// Catalog rows are documented as rotating as often as hourly.
const CATALOG_TTL: Duration = Duration::from_secs(300);
/// An ISRC maps to the same addon item for as long as the addon has it.
const RESOLVE_TTL: Duration = Duration::from_secs(3600);

/// Deliberately *not* cached: a stream URL may carry `expiresAt`, and serving a
/// stale URL from cache produces a dead playback attempt.
///
/// Eclipse's paging contract is "fewer than 100 items means the end of the row",
/// so this is the page size we count against.
const CATALOG_PAGE: u32 = 100;

/// A 404 on a catalog page or a detail lookup is the addon saying "nothing
/// here", not a failure. Anything else is a real error and must not be
/// conflated with an empty result.
fn is_404(e: &ClientError) -> bool {
    matches!(
        e,
        ClientError::Http { status: 404, .. } | ClientError::Json { status: 404, .. }
    )
}

/// Maps a 404 to `None` and leaves every other error alone.
fn optional<T>(result: Result<T, ClientError>) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if is_404(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[derive(Clone)]
pub struct EclipseService {
    pub client: sdks::RestClient,
    extra_query: Vec<(String, String)>,
}

impl EclipseService {
    /// `url` is the addon's base URL, with or without a trailing
    /// `/manifest.json`, and with any settings/auth query parameters attached.
    ///
    /// The suffix is stripped here rather than at the call site because the
    /// remaining path is significant: token-authenticated addons are installed
    /// as `https://host/{token}/manifest.json` and every subsequent route hangs
    /// off `https://host/{token}/`.
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url).map_err(|e| anyhow::anyhow!("{e}"))?;
        let extra_query: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut clean = parsed.clone();
        clean.set_query(None);
        clean.set_fragment(None);
        // Operate on the parsed path, not the whole string: a plain suffix
        // match would miss "…/manifest.json?token=abc", whose string does not
        // end with the suffix.
        let path = clean
            .path()
            .trim_end_matches('/')
            .trim_end_matches("/manifest.json")
            .to_string();
        clean.set_path(&path);
        let base = clean
            .as_str()
            .trim_end_matches('/')
            .to_string()
            + "/";
        Ok(Self {
            client: sdks::eclipse::client(&base)?,
            extra_query,
        })
    }

    /// Shares a 429 cooldown across every client built for the same addon.
    /// `from_url` builds a fresh `RestClient` on every call (addons are not
    /// cached), so without this each concurrent or sequential call starts
    /// with no memory of a prior 429 from the same addon.
    pub fn with_shared_rate_limit(mut self, limit: sdks::SharedRateLimit) -> Self {
        self.client = self
            .client
            .with_shared_rate_limit(limit);
        self
    }

    fn ep<EP: Endpoint + Clone>(&self, endpoint: EP) -> WithExtraQuery<EP> {
        WithExtraQuery {
            endpoint,
            extra: self
                .extra_query
                .clone(),
        }
    }

    pub async fn get_manifest(&self) -> Result<sdks::eclipse::Manifest> {
        Ok(self
            .client
            .execute(
                self.ep(sdks::eclipse::ManifestEndpoint)
                    .with_cache(MANIFEST_TTL),
            )
            .await?)
    }

    pub async fn search(&self, q: &str) -> Result<sdks::eclipse::SearchResponse> {
        Ok(self
            .client
            .execute(
                self.ep(sdks::eclipse::SearchEndpoint { q: q.to_string() })
                    .with_cache(SEARCH_TTL),
            )
            .await?)
    }

    /// Resolves a playable URL. Never cached — see [`MANIFEST_TTL`]'s neighbours.
    pub async fn get_stream(&self, id: &str) -> Result<sdks::eclipse::Stream> {
        Ok(self
            .client
            .execute(self.ep(sdks::eclipse::StreamEndpoint { id: id.to_string() }))
            .await?)
    }

    pub async fn get_album(&self, id: &str) -> Result<Option<sdks::eclipse::Album>> {
        optional(
            self.client
                .execute(
                    self.ep(sdks::eclipse::AlbumEndpoint { id: id.to_string() })
                        .with_cache(DETAIL_TTL),
                )
                .await,
        )
    }

    pub async fn get_artist(&self, id: &str) -> Result<Option<sdks::eclipse::Artist>> {
        optional(
            self.client
                .execute(
                    self.ep(sdks::eclipse::ArtistEndpoint { id: id.to_string() })
                        .with_cache(DETAIL_TTL),
                )
                .await,
        )
    }

    pub async fn get_playlist(
        &self,
        id: &str,
    ) -> Result<Option<sdks::eclipse::Playlist>> {
        optional(
            self.client
                .execute(
                    self.ep(sdks::eclipse::PlaylistEndpoint { id: id.to_string() })
                        .with_cache(DETAIL_TTL),
                )
                .await,
        )
    }

    /// The addon's own id for the recording carrying `isrc`, if it has it.
    ///
    /// A returned id is treated as an exact match, so a `None` here is strictly
    /// better than a guess.
    pub async fn resolve_isrc(&self, isrc: &str) -> Result<Option<String>> {
        let response = optional(
            self.client
                .execute(
                    self.ep(sdks::eclipse::ResolveIsrcEndpoint {
                        isrc: isrc.to_string(),
                    })
                    .with_cache(RESOLVE_TTL),
                )
                .await,
        )?;
        Ok(response
            .flatten()
            .and_then(|r| r.track_id)
            .filter(|id| !id.is_empty()))
    }

    /// The addon's own item for a recording described by identity rather than
    /// by id.
    pub async fn resolve(
        &self,
        isrc: Option<&str>,
        title: &str,
        artist: &str,
        duration_ms: Option<i64>,
    ) -> Result<Option<sdks::eclipse::CatalogItem>> {
        let response = optional(
            self.client
                .execute(
                    self.ep(sdks::eclipse::ResolveEndpoint {
                        isrc: isrc.map(str::to_string),
                        title: title.to_string(),
                        artist: artist.to_string(),
                        duration_ms,
                    })
                    .with_cache(RESOLVE_TTL),
                )
                .await,
        )?;
        Ok(response
            .flatten()
            .and_then(|r| r.item))
    }

    /// Pages a catalog row until the addon returns a short page, a 404, or an
    /// empty page.
    ///
    /// `skip` advances by whole pages of [`CATALOG_PAGE`] as the docs require,
    /// rather than by the number of items actually received: an addon that
    /// returns 100 items for page 0 and 100 for `skip=100` must not be asked
    /// for `skip=200` if it only ever had 150 — the short page ends it.
    pub async fn get_catalog_stream(
        &self,
        id: String,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::eclipse::CatalogItem> + Send>>> {
        let client = self
            .client
            .clone();
        let extra_query = self
            .extra_query
            .clone();

        let pages = stream::try_unfold(Some(0u32), move |state| {
            let client = client.clone();
            let extra_query = extra_query.clone();
            let id = id.clone();
            async move {
                let Some(skip) = state else {
                    return Ok(None);
                };
                let page = client
                    .execute(
                        WithExtraQuery {
                            endpoint: sdks::eclipse::CatalogEndpoint {
                                id: id.clone(),
                                // The first page is requested without `skip` so
                                // addons that only implement the bare route still
                                // serve it.
                                skip: (skip > 0).then_some(skip),
                            },
                            extra: extra_query,
                        }
                        .with_cache(CATALOG_TTL),
                    )
                    .await;
                match page {
                    Ok(response) => {
                        let count = response
                            .items
                            .len() as u32;
                        debug!(catalog = %id, skip, count, "eclipse catalog page");
                        // A short page is the documented end of the row.
                        let next = (count >= CATALOG_PAGE).then(|| skip + CATALOG_PAGE);
                        Ok(Some((response.items, next)))
                    }
                    Err(e) if is_404(&e) => {
                        debug!(catalog = %id, skip, "eclipse catalog page 404: end of row");
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
        })
        .take_while(|page: &Result<Vec<sdks::eclipse::CatalogItem>, ClientError>| {
            let keep = match page {
                Ok(_) => true,
                Err(e) => {
                    warn!(error = %e, "stopping eclipse catalog pagination");
                    false
                }
            };
            std::future::ready(keep)
        })
        .filter_map(|page| async move {
            page.ok()
                .map(stream::iter)
        })
        .flatten();

        Ok(Box::pin(pages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base URL whose path is unique to this call.
    ///
    /// The SDK response cache is process-wide and keyed by URL, while httpmock
    /// reuses ports across tests — so two tests that mock the same path on
    /// "their own" server can collide, and one reads the other's cached
    /// response. A distinct path prefix per test keeps the keys apart.
    fn scoped_base(server: &httpmock::MockServer) -> String {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}/t{n}", server.base_url())
    }

    fn mock_catalog_page(
        server: &httpmock::MockServer,
        scope: &str,
        skip: Option<u32>,
        count: usize,
        start: usize,
    ) {
        let items: Vec<_> = (start..start + count)
            .map(|i| {
                serde_json::json!({
                    "id": format!("t{i}"), "type": "track", "title": format!("Track {i}")
                })
            })
            .collect();
        let path = format!("{scope}/catalog/top");
        server.mock(|when, then| {
            let when = when.path(path);
            match skip {
                Some(s) => {
                    when.query_param("skip", s.to_string());
                }
                None => {
                    when.matches(|req| {
                        req.query_params
                            .as_ref()
                            .map(|q| {
                                !q.iter()
                                    .any(|(k, _)| k == "skip")
                            })
                            .unwrap_or(true)
                    });
                }
            };
            then.status(200)
                .json_body(serde_json::json!({"items": items}));
        });
    }

    /// The path component of a scoped base URL, for building mock paths.
    fn scope_of(base: &str) -> String {
        url::Url::parse(base)
            .unwrap()
            .path()
            .trim_end_matches('/')
            .to_string()
    }

    /// A page shorter than 100 items is the documented end of a row: the addon
    /// must not be asked for the next page.
    #[tokio::test]
    async fn catalog_stops_on_a_short_page() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        mock_catalog_page(&server, &scope, None, 3, 0);
        let next_page = server.mock(|when, then| {
            when.path(format!("{scope}/catalog/top"))
                .query_param("skip", "100");
            then.status(200)
                .json_body(serde_json::json!({"items": []}));
        });

        let svc = EclipseService::from_url(&base).unwrap();
        let items: Vec<String> = svc
            .get_catalog_stream("top".to_string())
            .await
            .unwrap()
            .map(|i| i.id)
            .collect()
            .await;

        assert_eq!(items, vec!["t0", "t1", "t2"]);
        next_page.assert_hits(0);
    }

    /// A 404 past the last page is a normal end-of-row signal, and must still
    /// yield the items collected before it.
    #[tokio::test]
    async fn catalog_stops_cleanly_on_404() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        mock_catalog_page(&server, &scope, None, 100, 0);
        server.mock(|when, then| {
            when.path(format!("{scope}/catalog/top"))
                .query_param("skip", "100");
            then.status(404);
        });

        let svc = EclipseService::from_url(&base).unwrap();
        let items: Vec<String> = svc
            .get_catalog_stream("top".to_string())
            .await
            .unwrap()
            .map(|i| i.id)
            .collect()
            .await;

        assert_eq!(items.len(), 100);
        assert_eq!(items[0], "t0");
    }

    /// A full page must be followed by a request for the next one, advancing by
    /// whole pages rather than by however many items arrived.
    #[tokio::test]
    async fn catalog_pages_until_exhausted() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        mock_catalog_page(&server, &scope, None, 100, 0);
        mock_catalog_page(&server, &scope, Some(100), 2, 100);

        let svc = EclipseService::from_url(&base).unwrap();
        let items: Vec<String> = svc
            .get_catalog_stream("top".to_string())
            .await
            .unwrap()
            .map(|i| i.id)
            .collect()
            .await;

        assert_eq!(items.len(), 102);
        assert_eq!(
            items
                .last()
                .unwrap(),
            "t101"
        );
    }

    /// Settings and auth parameters live in the configured URL and the docs
    /// require them on *every* request, including ones we build ourselves.
    #[tokio::test]
    async fn forwards_configured_query_params_to_every_request() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        let search = server.mock(|when, then| {
            when.path(format!("{scope}/search"))
                .query_param("q", "hello")
                .query_param("quality", "high");
            then.status(200)
                .json_body(serde_json::json!({"tracks": []}));
        });

        let svc = EclipseService::from_url(&format!("{base}/?quality=high")).unwrap();
        svc.search("hello")
            .await
            .unwrap();

        search.assert();
    }

    /// The docs say an addon URL may be given with or without `/manifest.json`.
    #[tokio::test]
    async fn accepts_a_manifest_url_as_the_base() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        let manifest = server.mock(|when, then| {
            when.path(format!("{scope}/manifest.json"));
            then.status(200)
                .json_body(serde_json::json!({
                    "id": "com.example.a", "name": "A", "version": "1.0.0",
                    "resources": ["search", "stream"],
                }));
        });

        let svc = EclipseService::from_url(&format!("{base}/manifest.json")).unwrap();
        assert_eq!(
            svc.get_manifest()
                .await
                .unwrap()
                .name,
            "A"
        );
        manifest.assert();
    }

    /// An addon may authenticate by putting a token in its URL path. Stripping
    /// `/manifest.json` must keep that prefix, or every subsequent request
    /// loses the token.
    #[tokio::test]
    async fn keeps_a_token_path_prefix_when_stripping_the_manifest_suffix() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        let search = server.mock(|when, then| {
            when.path(format!("{scope}/u/tok123/search"))
                .query_param("q", "hello");
            then.status(200)
                .json_body(serde_json::json!({"tracks": []}));
        });

        let svc = EclipseService::from_url(&format!("{base}/u/tok123/manifest.json"))
            .unwrap();
        svc.search("hello")
            .await
            .unwrap();

        search.assert();
    }

    /// A missing detail resource is an absent item, not a failure — the caller
    /// falls back to what it already knows.
    #[tokio::test]
    async fn missing_detail_is_none_not_an_error() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        server.mock(|when, then| {
            when.path(format!("{scope}/album/nope"));
            then.status(404);
        });

        let svc = EclipseService::from_url(&base).unwrap();
        assert!(
            svc.get_album("nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// A 5xx is a real failure and must not be reported as "no such album".
    #[tokio::test]
    async fn server_error_on_detail_is_an_error() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        server.mock(|when, then| {
            when.path(format!("{scope}/album/boom"));
            then.status(500);
        });

        let svc = EclipseService::from_url(&base).unwrap();
        assert!(
            svc.get_album("boom")
                .await
                .is_err()
        );
    }

    /// "I don't have that recording" arrives as a 404 or a null id; both mean
    /// the same thing, and neither is an error.
    #[tokio::test]
    async fn resolve_isrc_treats_absence_as_none() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        let path = format!("{scope}/resolve-isrc");
        server.mock(|when, then| {
            when.path(path.clone())
                .query_param("isrc", "USRC11903813");
            then.status(200)
                .json_body(serde_json::json!({"trackId": null}));
        });
        server.mock(|when, then| {
            when.path(path)
                .query_param("isrc", "GBAAW9500189");
            then.status(404);
        });

        let svc = EclipseService::from_url(&base).unwrap();
        assert_eq!(
            svc.resolve_isrc("USRC11903813")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            svc.resolve_isrc("GBAAW9500189")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn resolve_isrc_returns_the_addons_id() {
        let server = httpmock::MockServer::start();
        let base = scoped_base(&server);
        let scope = scope_of(&base);
        server.mock(|when, then| {
            when.path(format!("{scope}/resolve-isrc"));
            then.status(200)
                .json_body(serde_json::json!({"trackId": "516681628"}));
        });

        let svc = EclipseService::from_url(&base).unwrap();
        assert_eq!(
            svc.resolve_isrc("USUG12400123")
                .await
                .unwrap(),
            Some("516681628".to_string())
        );
    }
}
