use crate::db;
use crate::sdks;
use crate::sdks::CachedEndpoint;
use anyhow::Context;
use anyhow::{Result, anyhow};
//use futures::{StreamExt, stream};
use futures::stream::{self, Stream, StreamExt};
use futures_util::TryStreamExt;
use itertools::Itertools;
use std::pin::Pin;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use url::Url;

#[derive(Clone)]
pub struct AioService {
    pub client: sdks::RestClient,
    // to be clear,this is a searc for streams. Not meta
    pub search_client: sdks::RestClient<sdks::BasicAuth>,
}

impl AioService {
    pub fn from_url(url: &str) -> Result<Self> {
        let client = Self::get_aio(url)?;
        let search_client = Self::get_aio_search(url)?;
        Ok(Self {
            client,
            search_client,
        })
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
                .with_cache(Duration::from_secs(3600)),
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
                    ..Default::default()
                }
                .with_cache(Duration::from_secs(360)),
            )
            .await?
            .data
            .results)
    }

    pub async fn get_catalog_pages(
        &self,
        cat: &sdks::aio::Catalog,
    ) -> Result<Vec<sdks::aio::Meta>> {
        let results = stream::iter(0..500)
            .map(|page| {
                let client = &self.client;
                let kind = cat.kind.clone();
                let id = cat.id.clone();

                async move {
                    client
                        .execute(sdks::aio::CatalogEndpoint {
                            kind,
                            id,
                            search: None,
                            genre: None,
                            skip: Some(page),
                        })
                        .await
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;

        Ok(results
            .into_iter()
            .filter_map(|res| match res {
                Ok(response) => Some(response.metas),
                Err(e) => {
                    tracing::error!("Failed to fetch page: {}", e);
                    None
                }
            })
            .flatten()
            .unique_by(|x| x.id.clone())
            .collect())
    }

    pub async fn get_catalog_stream(
        &self,
        cat: &sdks::aio::Catalog,
    ) -> Pin<Box<dyn Stream<Item = sdks::aio::Meta> + Send>> {
        let client = self.client.clone();
        let kind = cat.kind.clone();
        let id = cat.id.clone();
        let mut page_size = 20;

        let pages = stream::iter(0..50)
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
            .buffer_unordered(10);

        let stream = pages
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

        Box::pin(stream)
    }
}
