use futures::stream::{StreamExt, FuturesUnordered};
use sqlx::{Postgres, Transaction, PgPool};
use sqlx::postgres::PgQueryBuilder;
use tracing;

#[derive(Clone)]
pub struct AppState {
    pub aio: AioClient, // Replace with your actual client type
    pub db: PgPool,
}

#[derive(Clone)]
pub struct ImportCatalogManager {
    state: AppState,
}

impl ImportCatalogManager {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn fetch_catalog_pages(&self, cat: &Catalog) -> Vec<db::Media> {
        let pages = (0..10).map(|page| {
            let state = self.state.clone();
            let skip = page;
            async move {
                state.aio.client.execute(sdks::aio::CatalogEndpoint {
                    kind: cat.kind.clone(),
                    id: cat.id.clone(),
                    search: None,
                    genre: None,
                    skip: Some(skip),
                }).await
            }
        });

        let results = FuturesUnordered::from_iter(pages)
            .buffer_unordered(5)
            .collect::<Vec<_>>()
            .await;

        results
            .into_iter()
            .filter_map(|res| res.ok())
            .flat_map(|response| response.metas.into_iter().map(|m| m.into()))
            .collect()
    }

    // Batch inserts media items with ON CONFLICT handling
    async fn batch_insert_media(&self, items: Vec<db::Media>) -> Result<(), sqlx::Error> {
        if items.is_empty() {
            return Ok(());
        }

        let mut tx = self.state.db.begin().await?;
        let mut query_builder: PgQueryBuilder = sqlx::QueryBuilder::new(
            "INSERT INTO media (imdb_id, kind, title, year, ...) "
        );
        query_builder.push_values(items.iter(), |mut b, item| {
            b.push_bind(&item.imdb_id)
             .push_bind(&item.kind)
             .push_bind(&item.title)
             .push_bind(&item.year);
            // Add other fields as needed
        });
        query_builder.push(" ON CONFLICT (imdb_id, kind) DO NOTHING");

        query_builder.build().execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    // Processes all catalogs in the manifest
    pub async fn import_all_catalogs(&self) -> Result<(), Box<dyn std::error::Error>> {
        let manifest = self.state.aio.get_manifest().await?;
        let tasks: FuturesUnordered<_> = manifest.catalogs.into_iter().map(|cat| {
            let manager = self.clone();
            async move {
                let items = manager.fetch_catalog_pages(&cat).await;
                if let Err(e) = manager.batch_insert_media(items).await {
                    tracing::error!("Failed to import catalog {}: {}", cat.id, e);
                } else {
                    tracing::info!("Imported catalog {}", cat.id);
                }
            }
        }).collect();

        tasks.collect::<()>().await;
        Ok(())
    }
}

// Example usage in your background task
async fn spawn_background_tasks(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    tokio::spawn(async move {
        let manager = ImportCatalogManager::new(state);
        if let Err(e) = manager.import_all_catalogs().await {
            tracing::error!("Import failed: {}", e);
        }
    });
    Ok(())
}