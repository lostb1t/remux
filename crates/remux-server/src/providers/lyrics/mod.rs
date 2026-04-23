use anyhow::Result;
use async_trait::async_trait;

mod lrclib;
pub use lrclib::LrcLibProvider;

use remux_sdks::remux::models::{LyricDto, RemoteLyricInfoDto};

pub struct LyricSearchRequest {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

#[async_trait]
pub trait LyricProvider: Send + Sync {
    fn name(&self) -> &'static str;
    /// Exact-match fetch; used by `GET /Audio/{id}/Lyrics`.
    async fn fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>>;
    /// Fuzzy search returning multiple candidates; used by `GET /Audio/{id}/RemoteSearch/Lyrics`.
    async fn search(&self, req: &LyricSearchRequest)
    -> Result<Vec<RemoteLyricInfoDto>>;
    /// Fetch a specific result by provider-scoped ID; used by `GET /Providers/Lyrics/{id}`.
    async fn get_by_id(&self, id: &str) -> Result<Option<LyricDto>>;
}

pub struct LyricService {
    providers: Vec<Box<dyn LyricProvider>>,
}

impl Default for LyricService {
    fn default() -> Self {
        Self {
            providers: vec![Box::new(LrcLibProvider::default())],
        }
    }
}

impl LyricService {
    pub async fn fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>> {
        for provider in &self.providers {
            match provider.fetch(req).await {
                Ok(Some(lyrics)) => return Ok(Some(lyrics)),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(provider = provider.name(), error = %e, "lyric provider fetch error");
                    continue;
                }
            }
        }
        Ok(None)
    }

    pub async fn search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let mut results = Vec::new();
        for provider in &self.providers {
            match provider.search(req).await {
                Ok(items) => results.extend(items),
                Err(e) => {
                    tracing::warn!(provider = provider.name(), error = %e, "lyric provider search error");
                }
            }
        }
        Ok(results)
    }

    /// `composite_id` is `{providerName}_{id}` (e.g. `lrclib_3396226`).
    pub async fn get_by_composite_id(
        &self,
        composite_id: &str,
    ) -> Result<Option<LyricDto>> {
        for provider in &self.providers {
            let prefix = format!("{}_", provider.name());
            if let Some(inner_id) = composite_id.strip_prefix(&prefix) {
                return provider.get_by_id(inner_id).await;
            }
        }
        Ok(None)
    }
}
