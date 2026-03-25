use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::sdks::CachedEndpoint;
use crate::{AppContext, db, sdks};

pub struct CatalogItemImportTask {
    catalog_id: Uuid,
    key: String,
    display_name: String,
}

impl CatalogItemImportTask {
    pub fn new(catalog_id: Uuid, name: &str) -> Self {
        Self {
            catalog_id,
            key: Self::task_key(catalog_id),
            display_name: format!("Import {}", name),
        }
    }

    pub fn task_key(catalog_id: Uuid) -> String {
        format!("catalogimport:{}", catalog_id)
    }
}

/// Try to ensure `meta.imdb_id` is populated before DB conversion.
///
/// Resolution order:
/// 1. Parse `meta.id` prefix — if it is an IMDb ID (`tt…`) we already have it.
/// 2. Call AIO `meta.resolve()` to fetch full metadata from the addon.
/// 3. TMDB detail lookup — if we have a TMDB id (from step 1 or `meta.moviedb_id`)
///    and a valid API key, call the TMDB Movie/Series endpoint which returns `imdb_id`.
///
/// Returns `true` if `meta.imdb_id` is set after all attempts, `false` otherwise.
async fn resolve_imdb_id<A: sdks::Auth + Clone>(
    meta: &mut crate::sdks::aio::Meta,
    aio: &crate::aio::AioService,
    tmdb_client: Option<&sdks::RestClient<A>>,
) -> bool {
    if meta.imdb_id.is_some() {
      return true;
    }

    let parsed = db::ExternalIds::from_aio_id(&meta.id);
    if let Some(ref imdb) = parsed.imdb {
        meta.imdb_id = Some(imdb.clone());
        return true;
    }
    // Keep any tmdb/tvdb id extracted from the prefix for step 3.
    let parsed_tmdb = parsed.tmdb;

    if meta.imdb_id.is_none() {
        if let Err(e) = meta.resolve(&aio.client).await {
            warn!(id = %meta.id, error = %e, "failed to resolve meta from AIO addon");
        }
        if meta.imdb_id.is_some() {
            return true;
        }
    }

    let tmdb_id = parsed_tmdb
        .or_else(|| meta.moviedb_id.map(|n| n as i64));

    if let (Some(client), Some(tid)) = (tmdb_client, tmdb_id) {
        let imdb = match meta.media_type {
            crate::sdks::aio::MediaType::Movie => {
                client
                    .execute(
                        sdks::tmdb::MovieEndpoint::new(tid)
                            .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                    .ok()
                    .and_then(|m| m.imdb_id)
            }
            crate::sdks::aio::MediaType::Series => {
                client
                    .execute(
                        sdks::tmdb::SeriesEndpoint::new(tid)
                            .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                    .ok()
                    .and_then(|s| s.external_ids)
                    .and_then(|e| e.imdb_id)
            }
            _ => None,
        };

        if let Some(imdb) = imdb {
            meta.imdb_id = Some(imdb);
            return true;
        }
    }

    false
}

#[async_trait]
impl Task for CatalogItemImportTask {
    fn key(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        &self.display_name
    }
    fn category(&self) -> &str {
        "Import"
    }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let aio = crate::aio::AioService::from_settings(&ctx.db).await?;

        // Build TMDB client once if an API key is configured; used as fallback in resolve_imdb_id.
        let tmdb_client = {
            let cfg = crate::db::Settings::get_config(&ctx.db).await.ok();
            cfg.as_ref()
                .and_then(|c| c.tmdb_api_key.as_deref())
                .filter(|k| !k.is_empty())
                .and_then(|key| {
                    sdks::RestClient::new("https://api.themoviedb.org/3/")
                        .ok()
                        .map(|c| c.with_auth(sdks::BearerAuth { token: key.to_string() }))
                })
        };

        let catalog = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                id: Some(vec![self.catalog_id]),
                kind: Some(vec![db::MediaKind::Catalog]),
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Catalog {} not found", self.catalog_id))?;

        let aio_id = catalog
            .media_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Catalog has no media_id"))?
            .to_string();

        let manifest = aio.get_manifest().await?;
        let manifest_cat = manifest
            .catalogs
            .iter()
            .find(|c| format!("{}:{}", c.kind, c.id) == aio_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("Catalog {} not found in AIO manifest", aio_id)
            })?;

        let global_max = crate::db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let max = catalog
            .collection_max_items
            .map(|n| n as usize)
            .unwrap_or(global_max);

        info!("importing catalog {} (max={})", aio_id, max);

        let catalog_id = catalog.id;
        let mut meta_stream = aio.get_catalog_stream(&manifest_cat).await?.chunks(500);
        let mut count = 0usize;

        while let Some(mut metas) = meta_stream.next().await {
            progress.set(count as f64 / max.max(1) as f64 * 100.0);

            let remaining = max.saturating_sub(count);
            if remaining == 0 {
                break;
            }
            metas = metas.into_iter().take(remaining).collect();

            let items: Vec<db::Media> = futures::stream::iter(
                metas.into_iter().unique_by(|meta| meta.id.clone()),
            )
            .then(|mut meta| {
                let aio = aio.clone();
                let tmdb_client = tmdb_client.as_ref();
                async move {
                        let resolved = resolve_imdb_id(&mut meta, &aio, tmdb_client).await;
                        if !resolved {
                            warn!(id = %meta.id, "could not resolve imdb_id via any method, skipping");
                            return vec![];
                        }
                    match db::aio_meta_to_medias(meta) {
                        Ok(mut items) => {
                            if let Some(top) = items.first_mut() {
                                top.parent_id = None;
                            }
                            items
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to convert metadata, skipping");
                            vec![]
                        }
                    }
                }
            })
            .flat_map(futures::stream::iter)
            .collect()
            .await;

            if items.is_empty() {
                break;
            }

            if let Err(e) = db::Media::upsert(&ctx.db, &items).await {
                error!("failed to import chunk: {}", e);
                continue;
            }

            let relations: Vec<db::MediaRelation> = items
                .iter()
                .filter(|m| m.parent_id.is_none())
                .map(|m| db::MediaRelation {
                    left_media_id: m.id,
                    right_media_id: catalog_id,
                    role: Some(db::RelationRole::Catalog),
                    ..Default::default()
                })
                .collect();

            if !relations.is_empty() {
                if let Err(e) = db::MediaRelation::upsert(&ctx.db, &relations).await {
                    error!("failed to upsert catalog relations: {}", e);
                }
            }

            count += items.len();
            if count >= max {
                break;
            }
        }

        info!("import complete for catalog {}: {} items", aio_id, count);

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}
