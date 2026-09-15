use crate::{
    sdks,
    sdks::{CachedEndpoint, ClientError, Endpoint, WithExtraQuery},
};
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
    extra_query: Vec<(String, String)>,
}

impl StremioService {
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url).map_err(|e| anyhow::anyhow!("{e}"))?;
        let extra_query: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut clean = parsed.clone();
        clean.set_query(None);
        let base = clean
            .as_str()
            .trim_end_matches('/')
            .to_string()
            + "/";
        Ok(Self {
            client: sdks::stremio::client(&base)?,
            extra_query,
        })
    }

    fn ep<EP: Endpoint + Clone>(&self, endpoint: EP) -> WithExtraQuery<EP> {
        WithExtraQuery {
            endpoint,
            extra: self
                .extra_query
                .clone(),
        }
    }

    pub async fn get_manifest(&self) -> Result<sdks::stremio::Manifest> {
        Ok(self
            .client
            .execute(
                self.ep(sdks::stremio::ManifestEndpoint)
                    .with_cache(Duration::from_secs(3600)),
            )
            .await?)
    }

    pub async fn get_meta(
        &self,
        media_type: sdks::stremio::MediaType,
        id: impl Into<String>,
    ) -> Result<sdks::stremio::Meta> {
        let id = id.into();
        let meta: Result<sdks::stremio::Meta> = remux_utils::retry! {
            attempts: 3,
            delay: 500,
            {
                let response = self
                    .client
                    .execute(
                        self.ep(sdks::stremio::MetaEndpoint {
                            media_type: media_type.clone(),
                            id: id.clone(),
                            season: None,
                            episode: None,
                        })
                        // Addons sometimes return a 503 as a successful Stremio
                        // meta payload. Do not cache that transient failure.
                        .with_cache(Duration::from_secs(3600))
                        .should_cache(cache_successful_meta),
                    )
                    .await?;
                if is_retryable_meta_error(&response.meta) {
                    Err(anyhow!("Stremio addon returned a retryable 5xx meta payload"))
                } else {
                    Ok(response.meta)
                }
            }
        };
        meta
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
                self.ep(sdks::stremio::CatalogEndpoint {
                    kind: catalog
                        .kind
                        .clone(),
                    id: catalog
                        .id
                        .clone(),
                    search: Some(q),
                    genre: None,
                    skip: None,
                })
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
                self.ep(sdks::stremio::StreamEndpoint {
                    kind: media_type,
                    id: id.into(),
                })
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
                self.ep(sdks::stremio::SubtitlesEndpoint {
                    media_type,
                    imdb_id: imdb_id.to_string(),
                    season,
                    episode,
                })
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
        page_concurrency: usize,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::stremio::Meta> + Send>>> {
        let client = self
            .client
            .clone();
        let extra_query = self
            .extra_query
            .clone();

        let t0 = Instant::now();
        let first_page = client
            .execute(WithExtraQuery {
                endpoint: sdks::stremio::CatalogEndpoint {
                    kind: kind.clone(),
                    id: id.clone(),
                    search: None,
                    genre: None,
                    skip: None,
                },
                extra: extra_query.clone(),
            })
            .await?;

        let page_size = first_page
            .metas
            .len() as u32;
        debug!(
            kind = %kind,
            id = %id,
            page_size,
            page_concurrency = page_concurrency.max(1),
            elapsed = ?t0.elapsed(),
            "catalog first page"
        );
        if page_size == 0 || !supports_skip {
            return Ok(Box::pin(stream::iter(first_page.metas)));
        }

        let first = stream::once(future::ready(Ok(first_page)));

        let rest = stream::iter(1..999u32)
            .map(move |page| {
                let client = client.clone();
                let kind = kind.clone();
                let id = id.clone();
                let extra_query = extra_query.clone();
                async move {
                    let skip = page * page_size;
                    let mut raw_response = None;
                    let result = client
                        .execute_observed(
                            WithExtraQuery {
                                endpoint: sdks::stremio::CatalogEndpoint {
                                    kind: kind.clone(),
                                    id: id.clone(),
                                    search: None,
                                    genre: None,
                                    skip: Some(skip),
                                },
                                extra: extra_query,
                            },
                            |status, body| {
                                raw_response = Some((status, body.to_string()))
                            },
                        )
                        .await;
                    match &result {
                        Ok(response) => tracing::info!(
                            kind = %kind,
                            id = %id,
                            page,
                            skip,
                            metas = response.metas.len(),
                            "catalog page fetched"
                        ),
                        Err(error) => {
                            let status = raw_response
                                .as_ref()
                                .map(|(status, _)| *status)
                                .unwrap_or_default();
                            let body = raw_response
                                .as_ref()
                                .map(|(_, body)| body.as_str())
                                .unwrap_or_default();
                            tracing::info!(
                                kind = %kind,
                                id = %id,
                                page,
                                skip,
                                status,
                                body = %body,
                                error = %error,
                                "catalog page fetch failed"
                            )
                        }
                    }
                    if let Ok(response) = &result
                        && response
                            .metas
                            .is_empty()
                    {
                        let status = raw_response
                            .as_ref()
                            .map(|(status, _)| *status)
                            .unwrap_or_default();
                        let body = raw_response
                            .as_ref()
                            .map(|(_, body)| body.as_str())
                            .unwrap_or_default();
                        tracing::info!(
                            kind = %kind,
                            id = %id,
                            page,
                            skip,
                            status,
                            body = %body,
                            "catalog page returned no metas"
                        );
                    }
                    result
                }
            })
            .buffered(page_concurrency.max(1));

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

fn cache_successful_meta(response: &sdks::stremio::MetaResponse) -> Option<Duration> {
    (!response
        .meta
        .is_error())
    .then_some(Duration::from_secs(3600))
}

fn is_retryable_meta_error(meta: &sdks::stremio::Meta) -> bool {
    meta.is_error()
        && meta
            .description
            .as_deref()
            .and_then(|description| {
                description
                    .trim_start()
                    .get(..3)
            })
            .and_then(|status| {
                status
                    .parse::<u16>()
                    .ok()
            })
            .is_some_and(|status| (500..600).contains(&status))
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
            .get_catalog_stream("movie".to_string(), "test".to_string(), true, 3)
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
            .get_catalog_stream("movie".to_string(), "test".to_string(), true, 3)
            .await
            .unwrap();
        let names: Vec<String> = stream
            .map(|m| m.id)
            .collect()
            .await;

        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }
}
