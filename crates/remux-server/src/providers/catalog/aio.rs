use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use super::{CatalogInfo, CatalogProvider};
use crate::{AppContext, aio, sdks};

pub struct AioCatalogProvider;

#[async_trait]
impl CatalogProvider for AioCatalogProvider {
    fn provider_id(&self) -> &'static str {
        "aio"
    }

    async fn list_catalogs(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let aio = aio::AioService::from_settings(&ctx.db).await?;
        let manifest = aio.get_manifest().await?;
        Ok(manifest
            .catalogs
            .into_iter()
            .filter(|c| !c.id.contains("search"))
            .map(|c| CatalogInfo {
                provider_catalog_id: format!("{}:{}", c.kind, c.id),
                name: c.name.trim().to_string(),
            })
            .collect())
    }

    async fn stream_items(
        &self,
        provider_catalog_id: &str,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = sdks::aio::Meta> + Send>>> {
        let aio = aio::AioService::from_settings(&ctx.db).await?;
        let manifest = aio.get_manifest().await?;
        let cat = manifest
            .catalogs
            .into_iter()
            .find(|c| format!("{}:{}", c.kind, c.id) == provider_catalog_id)
            .ok_or_else(|| {
                anyhow!(
                    "catalog '{}' not found in AIO manifest",
                    provider_catalog_id
                )
            })?;
        aio.get_catalog_stream(&cat).await
    }
}
