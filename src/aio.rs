use crate::db;
use crate::sdks;
use crate::sdks::CachedEndpoint;
use anyhow::Context;
use anyhow::{Result, anyhow};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct AioService {
    pub client: sdks::RestClient,
    // to be clear,this is a searc for streams. Not meta
    pub search_client: sdks::RestClient<sdks::BasicAuth>,
}

impl AioService {
    pub fn from_user(user: &db::User) -> Result<Self> {
        let client = Self::get_aio(user)?;
        let search_client = Self::get_aio_search(user)?;
        Ok(Self {
            client,
            search_client,
        })
    }

    fn get_aio(user: &db::User) -> Result<sdks::RestClient> {
        let base = user
            .aio_url
            .strip_suffix("manifest.json")
            .unwrap_or(user.aio_url.as_str())
            .to_string();

        let base = base.trim_end_matches('/').to_string() + "/";

        Ok(sdks::aio::client(&base)?)
    }

    fn get_aio_search(user: &db::User) -> Result<sdks::RestClient<sdks::BasicAuth>> {
        let mut url = Url::parse(&user.aio_url)?;

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
                    ..Default::default() //  })
                }
                .with_cache(Duration::from_secs(360)),
            )
            .await?
            .data
            .results)
    }
}
