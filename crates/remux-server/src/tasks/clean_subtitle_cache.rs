use anyhow::Result;
use async_trait::async_trait;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tracing::{info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

/// Extracted subtitles (and attachments) older than this are deleted. Same
/// age Jellyfin's "Clean Cache Directory" task uses for its cache folder.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A `.tmp` file is a half-written extraction; anything this old belongs to
/// a run that died (extractions time out after 10 minutes).
const MAX_TMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub struct CleanSubtitleCacheTask;

#[async_trait]
impl Task for CleanSubtitleCacheTask {
    fn key(&self) -> &str {
        "CleanSubtitleCache"
    }
    fn name(&self) -> &str {
        "Clean Subtitle Cache"
    }
    fn description(&self) -> &str {
        "Deletes subtitles extracted from media files that haven't been re-extracted in 30 days. They are extracted again on demand."
    }
    fn short_description(&self) -> &str {
        "Deletes extracted subtitles older than 30 days"
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
        let dir = ctx
            .config
            .data_dir
            .join("subtitle-cache");
        let (removed, bytes) = tokio::task::spawn_blocking(move || {
            clean_old_files(&dir, SystemTime::now(), MAX_AGE, MAX_TMP_AGE)
        })
        .await?;
        info!(removed, bytes, "cleaned subtitle cache");
        progress.set(100.0);
        Ok(())
    }
}

/// Deletes regular files in `dir` last written more than `max_age` before
/// `now` (`max_tmp_age` for `.tmp` leftovers). Returns (files, bytes) removed.
fn clean_old_files(
    dir: &Path,
    now: SystemTime,
    max_age: Duration,
    max_tmp_age: Duration,
) -> (usize, u64) {
    let mut removed = 0usize;
    let mut bytes = 0u64;
    for entry in super::iter_dir(dir) {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let limit = if entry
            .path()
            .extension()
            .is_some_and(|e| e == "tmp")
        {
            max_tmp_age
        } else {
            max_age
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|m| {
                now.duration_since(m)
                    .ok()
            });
        if age.is_none_or(|a| a <= limit) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                removed += 1;
                bytes += meta.len();
            }
            Err(e) => warn!(
                "failed to remove cached subtitle {}: {e}",
                entry
                    .path()
                    .display()
            ),
        }
    }
    (removed, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str, age: Duration) {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap();
        file.set_modified(SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn deletes_only_files_past_their_age() {
        let dir = tempfile::tempdir().unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        touch(dir.path(), "old.srt", 31 * day);
        touch(dir.path(), "fresh.ass", 2 * day);
        touch(dir.path(), "stale.ass.tmp", 2 * day);
        touch(dir.path(), "running.sup.tmp", Duration::from_secs(60));
        std::fs::create_dir(
            dir.path()
                .join("subdir"),
        )
        .unwrap();

        let (removed, bytes) =
            clean_old_files(dir.path(), SystemTime::now(), MAX_AGE, MAX_TMP_AGE);

        assert_eq!((removed, bytes), (2, 2));
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .file_name()
                    .into_string()
                    .unwrap()
            })
            .collect();
        left.sort();
        assert_eq!(left, ["fresh.ass", "running.sup.tmp", "subdir"]);
    }

    #[test]
    fn missing_directory_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (removed, _) = clean_old_files(
            &dir.path()
                .join("nope"),
            SystemTime::now(),
            MAX_AGE,
            MAX_TMP_AGE,
        );
        assert_eq!(removed, 0);
    }
}
