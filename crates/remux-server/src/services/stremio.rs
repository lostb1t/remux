use crate::sdks;
use crate::sdks::CachedEndpoint;
use anyhow::{Result, anyhow};
use futures::future;
use futures::stream::{self, Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;

#[derive(Clone)]
pub struct StremioService {
    pub client: sdks::RestClient,
}

impl StremioService {
    pub fn from_url(url: &str) -> Result<Self> {
        let base = url.trim_end_matches('/').to_string() + "/";
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
        id: String,
    ) -> Result<sdks::stremio::Meta> {
        Ok(self
            .client
            .execute(
                sdks::stremio::MetaEndpoint {
                    media_type,
                    id,
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
                    kind: catalog.kind.clone(),
                    id: catalog.id.clone(),
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
        id: String,
    ) -> Result<Vec<sdks::stremio::Stream>> {
        Ok(self
            .client
            .execute(
                sdks::stremio::StreamEndpoint {
                    kind: media_type,
                    id,
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
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::stremio::Meta> + Send>>> {
        let client = self.client.clone();

        let first_page = client
            .execute(sdks::stremio::CatalogEndpoint {
                kind: kind.clone(),
                id: id.clone(),
                search: None,
                genre: None,
                skip: None,
            })
            .await?;

        let page_size = first_page.metas.len() as u32;
        if page_size == 0 {
            return Ok(Box::pin(stream::empty()));
        }

        let first = stream::once(future::ready(Ok(first_page)));

        let rest = stream::iter(1..999u32)
            .map(move |page| {
                let client = client.clone();
                let kind = kind.clone();
                let id = id.clone();
                async move {
                    client
                        .execute(sdks::stremio::CatalogEndpoint {
                            kind,
                            id,
                            search: None,
                            genre: None,
                            skip: Some(page * page_size),
                        })
                        .await
                }
            })
            .buffered(3);

        let pages = first
            .chain(rest)
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
                        tracing::error!("Failed to fetch catalog page: {}", e);
                        None
                    }
                }
            })
            .flatten();

        Ok(Box::pin(pages))
    }
}
