use crate::{AppContext, sdks};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

mod aio;
pub use aio::AioCatalogProvider;

/// Provider-agnostic descriptor for a single remote catalog.
#[derive(Debug, Clone)]
pub struct CatalogInfo {
    /// Opaque catalog ID scoped to this provider (e.g. `"movie:top"` for AIO).
    /// Combined with `provider_id()` this forms the DB `media_id`: `"{provider}:{id}"`.
    pub provider_catalog_id: String,
    pub name: String,
}

/// A pluggable backend that exposes a list of remote catalogs and can stream
/// items from each one.
#[async_trait]
pub trait CatalogProvider: Send + Sync {
    /// Short stable identifier written as the `media_id` prefix (e.g. `"aio"`).
    /// Must be lowercase, URL-safe, and unique across providers.
    fn provider_id(&self) -> &'static str;

    /// List every catalog this provider exposes.
    async fn list_catalogs(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>>;

    /// Stream raw metadata items for one catalog.
    /// `provider_catalog_id` matches [`CatalogInfo::provider_catalog_id`].
    // TODO(music): replace sdks::aio::Meta with a provider-agnostic item type
    async fn stream_items(
        &self,
        provider_catalog_id: &str,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::aio::Meta> + Send>>>;
}

pub struct CatalogProviderManager {
    providers: Vec<Box<dyn CatalogProvider>>,
}

impl Default for CatalogProviderManager {
    fn default() -> Self {
        Self {
            providers: vec![Box::new(AioCatalogProvider)],
        }
    }
}

impl CatalogProviderManager {
    pub fn providers(&self) -> &[Box<dyn CatalogProvider>] {
        &self.providers
    }

    /// Find the provider that owns a given DB `media_id` (prefix match).
    pub fn provider_for_media_id(
        &self,
        media_id: &str,
    ) -> Option<&dyn CatalogProvider> {
        self.providers
            .iter()
            .find(|p| media_id.starts_with(&format!("{}:", p.provider_id())))
            .map(|p| p.as_ref())
    }

    /// Strip the provider prefix from a DB `media_id`, returning the
    /// `provider_catalog_id` understood by the owning provider.
    pub fn strip_prefix<'a>(
        &self,
        provider: &dyn CatalogProvider,
        media_id: &'a str,
    ) -> &'a str {
        media_id
            .strip_prefix(&format!("{}:", provider.provider_id()))
            .unwrap_or(media_id)
    }
}
