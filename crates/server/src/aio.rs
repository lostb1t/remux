use crate::db;
use crate::sdks;
use crate::sdks::CachedEndpoint;
use anyhow::Context;
use anyhow::{Result, anyhow};
//use futures::{StreamExt, stream};
use futures::future;
use futures::stream::{self, Stream, StreamExt};
use futures_util::TryStreamExt;
use itertools::Itertools;
use std::pin::Pin;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use url::Url;

/// Rewrite a media URL if it points to a Docker-internal service.
/// Creates an AioService from DB settings on-demand; returns the URL
/// unchanged if AIO is not configured.
pub async fn resolve_url(db: &sqlx::SqlitePool, url: &str) -> String {
    match AioService::from_settings(db).await {
        Ok(aio) => aio.rewrite_url(url),
        Err(_) => url.to_string(),
    }
}

#[derive(Clone)]
pub struct AioService {
    pub client: sdks::RestClient,
    // to be clear,this is a searc for streams. Not meta
    pub search_client: sdks::RestClient<sdks::BasicAuth>,
    /// The origin (scheme+host+port) of the user-configured aio_url.
    /// Used to rewrite AIOStreams-internal playback URLs so they are
    /// reachable from remux-server (which may be outside Docker).
    pub origin: String,
}

impl AioService {
    /// Build an AioService from the URL stored in the DB settings.
    /// Cheap to call — the HTTP response cache is process-global, so no cache
    /// misses occur even when the instance is recreated per-request.
    pub async fn from_settings(db: &sqlx::SqlitePool) -> Result<Self> {
        let url = crate::db::Settings::get_config(db)
            .await?
            .aio_url
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow!("AIO URL not configured — complete the setup wizard first")
            })?;
        Self::from_url(&url)
    }

    pub fn from_url(url: &str) -> Result<Self> {
        let client = Self::get_aio(url)?;
        let search_client = Self::get_aio_search(url)?;
        let parsed = Url::parse(url)?;
        let origin = parsed.origin().unicode_serialization();
        Ok(Self {
            client,
            search_client,
            origin,
        })
    }

    /// Rewrite an AIOStreams playback URL to use our configured origin.
    ///
    /// AIOStreams generates playback proxy URLs using its `BASE_URL` env var,
    /// which may be a Docker-internal hostname (e.g. `http://aiostreams:6006`).
    /// If we're outside that Docker network, those URLs are unreachable.
    /// Replace the origin with the one from the user-configured `aio_url`.
    ///
    /// Also rewrites other Docker-internal addon URLs (e.g. Comet) that share
    /// the same Docker network, since these are also unreachable from outside.
    pub fn rewrite_url(&self, url: &str) -> String {
        match Url::parse(url) {
            Ok(parsed) => {
                let url_origin = parsed.origin().unicode_serialization();
                if url_origin == self.origin {
                    return url.to_string(); // Already using our origin
                }
                // Only rewrite URLs that look Docker-internal:
                // no dots in host means it's a Docker service name (e.g. "comet", "aiostreams")
                let is_docker_internal = parsed.host_str()
                    .map(|h| !h.contains('.') && h != "localhost")
                    .unwrap_or(false);
                if !is_docker_internal {
                    return url.to_string();
                }
                // Replace the origin portion of the URL
                let rest = &url[url_origin.len()..];
                format!("{}{}", self.origin, rest)
            }
            Err(_) => url.to_string(),
        }
    }

    //pub fn from_user(user: &db::User) -> Result<Self> {
    //    let base = user
    //        .aio_url;
    //    let client = Self::get_aio(base)?;
    //    let search_client = Self::get_aio_search(base)?;
    //    Ok(Self {
    //        client,
    //        search_client,
    //    })
    // }

    fn get_aio(url: &str) -> Result<sdks::RestClient> {
        let base = url.trim_end_matches('/').to_string() + "/";
        Ok(sdks::aio::client(&base)?)
    }

    fn get_aio_search(base: &str) -> Result<sdks::RestClient<sdks::BasicAuth>> {
        let mut url = Url::parse(&base)?;
        let segments: Vec<String> = url
            .path_segments()
            .ok_or_else(|| anyhow!("aio_url has no path segments"))?
            .map(|s| s.to_string())
            .collect();

        if segments.len() < 3 {
            return Err(anyhow!(
                "invalid aio_url format: expected /stremio/<username>/<password>/..."
            ));
        }

        // if segments[0] != "stremio" {
        //     return Err(anyhow!(
        //         "invalid aio_url format: expected first segment to be 'stremio', got '{}'",
        //         segments[0]
        //     ));
        // }

        let username = segments[1].clone();
        let password = segments[2].clone();

        // Point at the authenticated API base
        url.set_path("/api/v1");
        url.set_query(None);
        url.set_fragment(None);

        let search_url = url.to_string();

        Ok(sdks::aio::search_client(&search_url, username, password)?)
    }

    pub async fn get_manifest(&self) -> Result<sdks::aio::Manifest> {
        Ok(self
            .client
            .execute(sdks::aio::ManifestEndpoint.with_cache(Duration::from_secs(3600)))
            .await?)
    }

    pub async fn get_meta(
        &self,
        media_type: sdks::aio::MediaType,
        id: String,
    ) -> Result<sdks::aio::Meta> {
        Ok(self
            .client
            .execute(
                sdks::aio::MetaEndpoint {
                    media_type,
                    id,
                    season: None,
                    episode: None,
                }
                .with_cache(Duration::from_secs(360)),
            )
            .await?
            .meta)
    }

    pub async fn search(
        &self,
        media_type: sdks::aio::MediaType,
        q: String,
    ) -> Result<Vec<sdks::aio::Meta>> {
        let catalog = self
            .get_manifest()
            .await?
            .get_search_catalog(&media_type.to_string())
            .unwrap();
        Ok(self
            .client
            .execute(
                sdks::aio::CatalogEndpoint {
                    kind: catalog.kind.clone(),
                    id: catalog.id.clone(),
                    search: Some(q.clone()),
                    genre: None,
                    skip: None, //skip: Some(skip),
                }
                .with_cache(Duration::from_secs(60)),
            )
            .await?
            .metas)
    }

    pub async fn get_stream(
        &self,
        media_type: sdks::aio::MediaType,
        id: String,
        stream_id: String,
    ) -> Result<sdks::aio::Stream> {
        let streams = self.get_streams(media_type, id).await?;

        let stream = streams
            .into_iter()
            .find(|x| x.id() == stream_id)
            .context("no stream")?;

        Ok(stream)
    }

    pub async fn get_streams(
        &self,
        media_type: sdks::aio::MediaType,
        id: String,
    ) -> Result<Vec<sdks::aio::Stream>> {
        Ok(self
            .search_client
            .execute(
                sdks::aio::Search {
                    kind: media_type.into(),
                    id,
                    format: true,
                }
                .with_cache(Duration::from_secs(360)),
            )
            .await?
            .data
            .results)
    }

    pub async fn get_subtitles(
        &self,
        media_type: sdks::aio::MediaType,
        imdb_id: &str,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Vec<sdks::aio::Subtitle>> {
        Ok(self
            .client
            .execute(
                sdks::aio::SubtitlesEndpoint {
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
        cat: &sdks::aio::Catalog,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::aio::Meta> + Send>>> {
        let client = self.client.clone();
        let kind = cat.kind.clone();
        let id = cat.id.clone();

        // get page size. theres no default
        let page_size = client
            .execute(sdks::aio::CatalogEndpoint {
                kind: kind.clone(),
                id: id.clone(),
                search: None,
                genre: None,
                skip: None,
            })
            .await?
            .metas
            .len() as u32;

        let pages = stream::iter(0..999)
            .map(move |page| {
                let client = client.clone();
                let kind = kind.clone();
                let id = id.clone();
                async move {
                    client
                        .execute(sdks::aio::CatalogEndpoint {
                            kind,
                            id,
                            search: None,
                            genre: None,
                            skip: Some(page * page_size),
                        })
                        .await
                }
            })
            .buffered(10)
            .take_while(|result| {
                future::ready(
                    result
                        .as_ref()
                        .map(|response| !response.metas.is_empty())
                        .unwrap_or(true),
                )
            })
            .filter_map(|result| async move {
                match result {
                    Ok(response) => Some(stream::iter(response.metas)),
                    Err(e) => {
                        tracing::error!("Failed to fetch page: {}", e);
                        None
                    }
                }
            })
            .flatten();

        Ok(Box::pin(pages))
    }
}
