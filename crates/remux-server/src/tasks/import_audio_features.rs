use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct ImportAudioFeaturesTask;

#[async_trait]
impl Task for ImportAudioFeaturesTask {
    fn key(&self) -> &str {
        "ImportAudioFeatures"
    }
    fn name(&self) -> &str {
        "Import Audio Features"
    }
    fn description(&self) -> &str {
        "Downloads a Spotify audio-feature dataset and matches tracks in the library, enabling acoustic-similarity music recommendations."
    }
    fn short_description(&self) -> &str {
        "Downloads + matches audio features for music mixing"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Library
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let cfg = crate::db::Settings::get_config(&ctx.db).await?;
        let url = cfg
            .feature_dataset_url
            .as_deref()
            .unwrap_or("");
        if url.is_empty() {
            return Err(anyhow::anyhow!("No FeatureDatasetUrl configured"));
        }
        progress.set(1.0);
        tracing::info!("ImportAudioFeatures: starting import from {}", url);
        let st = crate::api::import_features::do_import(&ctx.db, url).await?;
        progress.set(99.0);
        tracing::info!(
            "ImportAudioFeatures: {} of {} tracks matched",
            st.matched,
            st.total_tracks
        );
        progress.set(100.0);
        Ok(())
    }
}
