use anyhow::Result;
use async_trait::async_trait;
#[cfg(unix)]
use libc;
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
        let active: HashSet<String> = ctx
            .sessions
            .active_session_ids()
            .into_iter()
            .collect();
        let base = ctx
            .sessions
            .base_dir();
        let mut removed = 0usize;

        for entry in super::iter_dir(base) {
            let name = entry
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !active.contains(&name) {
                // Kill any orphaned ffmpeg process before removing the dir.
                #[cfg(unix)]
                if let Ok(pid_str) = std::fs::read_to_string(
                    entry
                        .path()
                        .join(".pid"),
                ) {
                    if let Ok(pid) = pid_str
                        .trim()
                        .parse::<libc::pid_t>()
                    {
                        if pid > 0 {
                            unsafe {
                                libc::kill(pid, libc::SIGCONT);
                                libc::kill(pid, libc::SIGKILL);
                            }
                        }
                    }
                }
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    tracing::warn!(
                        "failed to remove transcode dir {}: {e:#}",
                        entry
                            .path()
                            .display()
                    );
                } else {
                    removed += 1;
                }
            }
        }
        info!(removed, "cleaned orphaned transcode dirs");

        progress.set(50.0);

        // Collect torrent IDs currently being streamed by active sessions so we
        // don't pull the rug out from under an in-progress playback.
        let mut active_torrent_ids = HashSet::new();
        for session in ctx
            .sessions
            .get_all()
        {
            if let Some(tc) = session.transcode {
                let input_url = tc
                    .read()
                    .await
                    .input_url
                    .clone();
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
