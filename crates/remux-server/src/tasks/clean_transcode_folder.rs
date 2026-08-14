use anyhow::Result;
use async_trait::async_trait;
#[cfg(unix)]
use libc;
use std::{collections::HashSet, sync::Arc};
use tracing::{info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
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
        "Deletes temporary files left over from transcoding sessions, and orphaned Torznab/rqbit torrent downloads no longer referenced by an active session."
    }
    fn short_description(&self) -> &str {
        "Deletes leftover transcode temp files and orphaned torrent downloads"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
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
                    warn!(
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

        // Also sweep any torrents left over from sessions that ended without
        // going through the normal stop path (e.g. a crash). The common case —
        // cleanup right after a session stops — is handled eagerly in
        // `api::session::report_playback_stopped`, so this is a safety net.
        crate::torrent::cleanup_unused(&ctx).await;

        progress.set(100.0);
        Ok(())
    }
}
