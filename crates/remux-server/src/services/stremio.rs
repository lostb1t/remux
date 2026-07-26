use crate::{sdks, sdks::CachedEndpoint, sdks::ClientError};
use anyhow::{Result, anyhow};
use futures::{
    future,
    stream::{self, Stream, StreamExt},
};
use std::{
    pin::Pin,
    time::{Duration, Instant},
};
use tracing::{debug, error};

/// A 404 on a paginated catalog page is the addon's normal "no more pages"
/// signal, not an error — some addons (e.g. fankai) 404 past the last page
/// instead of returning an empty array. Any other error (5xx, rate limit,
/// network blip) is a real problem and must not be conflated with reaching
/// the end of the catalog.
fn is_404(e: &ClientError) -> bool {
    matches!(
        e,
        ClientError::Http { status: 404, .. } | ClientError::Json { status: 404, .. }
    )
}

#[derive(Clone)]
pub struct StremioService {
    pub client: sdks::RestClient,
}

impl StremioService {
    pub fn from_url(url: &str) -> Result<Self> {
        let base = url
            .trim_end_matches('/')
            .to_string()
            + "/";
        Ok(Self {
            client: sdks::stremio::client(&base)?,
        })
    }

    pub async fn get_manifest(&self) -> Result<sdks::stremio::Manifest> {
        Ok(self
            .client
            .execute(
                sdks::stremio::ManifestEndpoint.with_cache(Duration::from_secs(3600)),
            )
            .await?)
    }

    pub async fn get_meta(
        &self,
        media_type: sdks::stremio::MediaType,
        id: impl Into<String>,
    ) -> Result<sdks::stremio::Meta> {
        Ok(self
            .client
            .execute(
                sdks::stremio::MetaEndpoint {
                    media_type,
                    id: id.into(),
                    season: None,
                    episode: None,
                }
                .with_cache(Duration::from_secs(3600)),
            )
            .await?
            .meta)
    }

    pub async fn search(
        &self,
        media_type: sdks::stremio::MediaType,
        q: String,
    ) -> Result<Vec<sdks::stremio::Meta>> {
        let catalog = self
            .get_manifest()
            .await?
            .get_search_catalog(&media_type.to_string())
            .ok_or_else(|| anyhow!("no search catalog for type {}", media_type))?;
        Ok(self
            .client
            .execute(
                sdks::stremio::CatalogEndpoint {
                    kind: catalog
                        .kind
                        .clone(),
                    id: catalog
                        .id
                        .clone(),
                    search: Some(q),
                    genre: None,
                    skip: None,
                }
                .with_cache(Duration::from_secs(60)),
            )
            .await?
            .metas)
    }

    pub async fn get_streams(
        &self,
        media_type: sdks::stremio::MediaType,
        id: impl Into<String>,
    ) -> Result<Vec<sdks::stremio::Stream>> {
        Ok(self
            .client
            .execute(
                sdks::stremio::StreamEndpoint {
                    kind: media_type,
                    id: id.into(),
                }
                .with_cache(Duration::from_secs(300)),
            )
            .await?
            .streams)
    }

    pub async fn get_subtitles(
        &self,
        media_type: sdks::stremio::MediaType,
        imdb_id: &str,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Vec<sdks::stremio::Subtitle>> {
        Ok(self
            .client
            .execute(
                sdks::stremio::SubtitlesEndpoint {
                    media_type,
                    imdb_id: imdb_id.to_string(),
                    season,
                    episode,
                }
                .with_cache(Duration::from_secs(86_400)),
            )
            .await?
            .subtitles)
    }

    pub async fn get_catalog_stream(
        &self,
        kind: String,
        id: String,
        supports_skip: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::stremio::Meta> + Send>>> {
        let client = self
            .client
            .clone();

        let t0 = Instant::now();
        let first_page = client
            .execute(sdks::stremio::CatalogEndpoint {
                kind: kind.clone(),
                id: id.clone(),
                search: None,
                genre: None,
                skip: None,
            })
            .await?;

        let page_size = first_page
            .metas
            .len() as u32;
        debug!(kind = %kind, id = %id, page_size, elapsed = ?t0.elapsed(), "catalog first page");
        if page_size == 0 || !supports_skip {
            return Ok(Box::pin(stream::iter(first_page.metas)));
        }

        let first = stream::once(future::ready(Ok(first_page)));

        let rest = stream::iter(1..999u32)
            .map(move |page| {
                let client = client.clone();
                let kind = kind.clone();
                let id = id.clone();
                async move {
                    let t = Instant::now();
                    let result = client
                        .execute(sdks::stremio::CatalogEndpoint {
                            kind: kind.clone(),
                            id: id.clone(),
                            search: None,
                            genre: None,
                            skip: Some(page * page_size),
                        })
                        .await;
                    result
                }
            })
            .buffered(3);

        let pages = first
            .chain(rest)
            // `take_while` drops the first item it rejects rather than passing it
            // downstream, so the 404-vs-error distinction has to happen here — by
            // the time a later `filter_map`/`map` would see the `Err`, take_while
            // has already stopped the stream without it.
            .take_while(|result| {
                future::ready(match result {
                    Ok(response) => !response
                        .metas
                        .is_empty(),
                    Err(e) if is_404(e) => {
                        debug!(
                            "stopping catalog pagination: reached end of catalog (404)"
                        );
                        false
                    }
                    Err(e) => {
                        error!(
                            "stopping catalog pagination due to unexpected error: {}",
                            e
                        );
                        false
                    }
                })
            })
            .filter_map(|result| async move {
                match result {
                    Ok(response) => Some(stream::iter(response.metas)),
                    Err(_) => None,
                }
            })
            .flatten();

        Ok(Box::pin(pages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_404_matches_only_404_status() {
        assert!(is_404(&ClientError::Http {
            status: 404,
            message: "not found".to_string(),
            endpoint: None,
            body: None,
        }));
        assert!(!is_404(&ClientError::Http {
            status: 500,
            message: "server error".to_string(),
            endpoint: None,
            body: None,
        }));
        assert!(!is_404(&ClientError::RateLimited {
            retry_after_secs: 30
        }));
    }

    fn mock_page(server: &httpmock::MockServer, path: &str, names: &[&str]) {
        let metas: Vec<_> = names
            .iter()
            .map(|n| serde_json::json!({"id": n, "type": "movie", "name": n}))
            .collect();
        server.mock(|when, then| {
            when.path(path);
            then.status(200)
                .json_body(serde_json::json!({"metas": metas}));
        });
    }

    /// Regression test for the take_while/filter_map bug: take_while drops the
    /// first item it rejects instead of passing it downstream, so a 404 on page
    /// 2 must still yield page 1's items and stop cleanly there.
    #[tokio::test]
    async fn get_catalog_stream_stops_cleanly_on_404() {
        let server = httpmock::MockServer::start();
        mock_page(&server, "/catalog/movie/test.json", &["a", "b"]);
        server.mock(|when, then| {
            when.path("/catalog/movie/test/skip=2.json");
            then.status(404);
        });

        let svc = StremioService::from_url(&server.base_url()).unwrap();
        let stream = svc
            .get_catalog_stream("movie".to_string(), "test".to_string(), true)
            .await
            .unwrap();
        let names: Vec<String> = stream
            .map(|m| m.id)
            .collect()
            .await;

        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    /// Same as above but for a non-404 error (5xx) — must still stop cleanly
    /// rather than hang or panic, even though it's not the expected
    /// end-of-catalog signal.
    #[tokio::test]
    async fn get_catalog_stream_stops_cleanly_on_non_404_error() {
        let server = httpmock::MockServer::start();
        mock_page(&server, "/catalog/movie/test.json", &["a", "b"]);
        server.mock(|when, then| {
            when.path("/catalog/movie/test/skip=2.json");
            then.status(500);
        });

        let svc = StremioService::from_url(&server.base_url()).unwrap();
        let stream = svc
            .get_catalog_stream("movie".to_string(), "test".to_string(), true)
            .await
            .unwrap();
        let names: Vec<String> = stream
            .map(|m| m.id)
            .collect()
            .await;

        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }
}
