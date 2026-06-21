use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use tracing::{debug, error, info, warn};

use crate::{
    AppContext,
    db::{self, Media, MediaKind},
    stream::{StreamDescriptor, StreamInfo},
};
use remux_sdks::remux::IntroOrder;

static VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "m4v"];

/// Scan `intro_dir`, upsert each video file as a `MediaKind::Intro` item,
/// and remove stale Intro items whose files no longer exist.
/// Resets `intro_idx` to 0 on every call.
pub async fn sync_intros(ctx: &AppContext, intro_idx: &AtomicUsize) -> Result<()> {
    let opts = db::Settings::get_intro_config(&ctx.db).await?;

    let existing = all_intros(&ctx.db).await?;

    let Some(dir_str) = opts
        .intro_dir
        .as_deref()
    else {
        // Intros disabled — remove all Intro items.
        for m in existing {
            if let Err(e) = Media::delete(&ctx.db, &m.id).await {
                warn!(id = %m.id, err = ?e, "failed to delete stale intro");
            } else {
                info!(id = %m.id, "deleted intro (intros disabled)");
            }
        }
        return Ok(());
    };

    let dir = Path::new(dir_str);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            error!(dir = %dir.display(), err = ?e, "failed to read intro dir");
            return Err(e.into());
        }
    };

    let mut found_ids = std::collections::HashSet::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if !VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                warn!(path = %path.display(), err = ?e, "cannot canonicalize intro path");
                continue;
            }
        };
        let path_str = canonical
            .to_string_lossy()
            .into_owned();
        let id = remux_utils::get_stable_uuid(path_str.clone());
        found_ids.insert(id);

        let title = canonical
            .file_stem()
            .map(|s| {
                s.to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "Intro".to_string());

        let url = path_str.clone();
        let probe_result = tokio::task::spawn_blocking(move || {
            crate::transcode::probing::probe_media(&url)
        })
        .await
        .ok()
        .and_then(|r| r.ok());

        let (runtime, probe_data) = match probe_result {
            Some((source_info, _segments)) => {
                let secs = source_info
                    .run_time_ticks
                    .map(|t| t / 10_000_000);
                debug!(path = %path_str, runtime_secs = ?secs, "probed intro");
                (secs, Some(source_info))
            }
            None => {
                warn!(path = %path_str, "ffprobe failed for intro, inserting without probe data");
                (None, None)
            }
        };

        let mut media = Media {
            id,
            title: title.clone(),
            kind: MediaKind::Intro,
            runtime,
            probe_data,
            stream_info: Some(StreamInfo {
                descriptor: StreamDescriptor::Local(canonical.clone()),
                filename: Some(
                    canonical
                        .file_name()
                        .map(|n| {
                            n.to_string_lossy()
                                .into_owned()
                        })
                        .unwrap_or_default(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };

        if let Err(e) = media
            .save(&ctx.db)
            .await
        {
            warn!(path = %path_str, err = ?e, "failed to upsert intro");
        } else {
            debug!(id = %id, path = %path_str, "upserted intro");
        }
    }

    // Remove stale Intro items whose files are no longer present.
    for m in existing {
        if found_ids.contains(&m.id) {
            continue;
        }
        if let Err(e) = Media::delete(&ctx.db, &m.id).await {
            warn!(id = %m.id, err = ?e, "failed to delete stale intro");
        } else {
            info!(id = %m.id, "deleted stale intro item");
        }
    }

    intro_idx.store(0, Ordering::Relaxed);
    Ok(())
}

/// Fetch all `MediaKind::Intro` items from the database.
pub async fn all_intros(db: &sqlx::SqlitePool) -> Result<Vec<Media>> {
    let items = sqlx::query_as::<_, Media>(
        "SELECT * FROM media WHERE kind = 'intro' ORDER BY title ASC",
    )
    .fetch_all(db)
    .await?;
    Ok(items)
}

/// Pick one intro from the list based on the configured order.
pub fn pick_intro<'a>(
    intros: &'a [Media],
    order: IntroOrder,
    idx: &AtomicUsize,
) -> Option<&'a Media> {
    if intros.is_empty() {
        return None;
    }
    match order {
        IntroOrder::Random => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            Some(&intros[seed % intros.len()])
        }
        IntroOrder::Sequential => {
            let i = idx.fetch_add(1, Ordering::Relaxed) % intros.len();
            Some(&intros[i])
        }
    }
}
