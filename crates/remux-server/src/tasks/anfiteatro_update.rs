use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct EnsureAnfiteatroTask;

#[async_trait]
impl Task for EnsureAnfiteatroTask {
    fn key(&self) -> &str {
        "EnsureAnfiteatro"
    }

    fn name(&self) -> &str {
        "Ensure Anfiteatro Web Client"
    }

    fn category(&self) -> &str {
        "Maintenance"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: std::sync::Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let Some(web_paths) = &ctx.web_paths else {
            return Ok(());
        };

        let target_path = &web_paths.anfiteatro_web_path;
        let index_html = Path::new(target_path).join("index.html");

        if index_html.exists() {
            tracing::info!("Anfiteatro web client already present at {target_path}");
            return Ok(());
        }

        let bundled_path = Path::new("/app/anfiteatro-web");
        if bundled_path.join("index.html").exists() {
            tracing::info!("Copying bundled Anfiteatro web client to {target_path}...");
            progress.set(50.0);
            crate::api::anfi::copy_dir_recursive(bundled_path, Path::new(target_path))
                .map_err(|e| anyhow::anyhow!("failed to copy bundled client: {e}"))?;
            tracing::info!(
                "Anfiteatro web client successfully copied to {target_path}"
            );
            progress.set(100.0);
            return Ok(());
        }

        tracing::info!("Anfiteatro web client missing; downloading latest release...");
        progress.set(10.0);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("remux-server")
            .build()?;

        let latest = crate::api::anfi::fetch_latest_anfiteatro_head(&client)
            .await
            .map_err(|e| anyhow::anyhow!("failed to fetch latest head: {e}"))?;

        progress.set(30.0);

        let archive_bytes =
            crate::api::anfi::download_anfiteatro_archive(&client, &latest.commit_sha)
                .await
                .map_err(|e| anyhow::anyhow!("failed to download archive: {e}"))?;

        progress.set(70.0);

        crate::api::anfi::install_archive_to_path(
            &archive_bytes,
            Path::new(target_path),
        )
        .map_err(|e| anyhow::anyhow!("failed to install archive: {e}"))?;

        crate::api::anfi::write_local_commit_marker(
            Path::new(target_path),
            &latest.commit_sha,
        )
        .map_err(|e| anyhow::anyhow!("failed to write commit marker: {e}"))?;

        tracing::info!("Anfiteatro web client successfully installed to {target_path}");
        progress.set(100.0);
        Ok(())
    }
}
