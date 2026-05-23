use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct CleanTranscodeFolderTask;

#[async_trait]
impl Task for CleanTranscodeFolderTask {
    fn key(&self) -> &str {
        "CleanTranscodeFolder"
    }
    fn name(&self) -> &str {
        "Clean Transcode Folder"
    }
    fn description(&self) -> &str {
        "Deletes temporary files left over from transcoding sessions."
    }
    fn short_description(&self) -> &str {
        "Deletes leftover temp transcode files"
    }
    fn category(&self) -> &str {
        "Maintenance"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        // --- transcode dirs ---
        let active: HashSet<String> =
            ctx.sessions.active_session_ids().into_iter().collect();
        let base = ctx.sessions.base_dir();
        let mut removed = 0usize;

        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !active.contains(&name) {
                    if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                        tracing::warn!(
                            "failed to remove transcode dir {}: {e:#}",
                            entry.path().display()
                        );
                    } else {
                        removed += 1;
                    }
                }
            }
        }
        info!(removed, "cleaned orphaned transcode dirs");

        progress.set(50.0);

        // --- torrents ---
        // Collect torrent IDs currently being streamed by active sessions so we
        // don't pull the rug out from under an in-progress playback.
        let mut active_torrent_ids = HashSet::new();
        for session in ctx.sessions.get_all() {
            if let Some(tc) = session.transcode {
                let input_url = tc.read().await.input_url.clone();
                if let Some(id) =
                    crate::torrent::TorrentManager::torrent_id_from_url(&input_url)
                {
                    active_torrent_ids.insert(id);
                }
            }
        }

        let deleted = ctx
            .torrent
            .delete_unused_with_files(&active_torrent_ids)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("failed to clean torrents: {e:#}");
                0
            });
        info!(deleted, "cleaned torrent sessions");

        progress.set(100.0);
        Ok(())
    }
}
