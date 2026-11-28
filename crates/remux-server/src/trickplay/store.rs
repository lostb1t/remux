//! A set's folder: its sheets, its markers, and freeing disk.

use super::*;

/// The number of the last sheet `pictures` fill.
pub fn last_sheet(pictures: u64) -> u64 {
    pictures.saturating_sub(1) / u64::from(TILE_WIDTH * TILE_HEIGHT)
}

/// How many sheets `pictures` fill.
pub(super) fn expected_sheets(pictures: u64) -> u32 {
    u32::try_from(last_sheet(pictures) + 1).unwrap_or(u32::MAX)
}

/// The sheet holding position `position_s`, at most the last of `expected`.
pub(super) fn sheet_at(position_s: u64, expected: u32) -> u32 {
    u32::try_from(position_s / SHEET_SECS)
        .unwrap_or(u32::MAX)
        .min(expected.saturating_sub(1))
}

/// What a set's folder holds, as a pass reads it: the whole sheets on disk,
/// and whether a run has reached the end of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SetState {
    pub(super) sheets: BTreeSet<u32>,
    pub(super) end: bool,
}

/// Whether a sheet is a whole JPEG: not empty, starting with FF D8 and ending
/// with FF D9. A sheet left empty or cut short by a power loss is not, and is
/// made again.
pub(super) fn sheet_whole(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 2];
    let mut tail = [0u8; 2];
    file.metadata()
        .is_ok_and(|m| m.len() >= 4)
        && file
            .read_exact(&mut head)
            .is_ok()
        && head == [0xFF, 0xD8]
        && file
            .seek(SeekFrom::End(-2))
            .is_ok()
        && file
            .read_exact(&mut tail)
            .is_ok()
        && tail == [0xFF, 0xD9]
}

/// Reads a set's folder: its whole sheets and the `end` marker.
pub(super) fn read_set(dir: &Path) -> SetState {
    let mut state = SetState {
        end: dir
            .join(END_MARKER)
            .exists(),
        ..Default::default()
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return state;
    };
    for entry in entries.flatten() {
        let Some(n) = entry
            .file_name()
            .to_str()
            .and_then(parse_sheet)
        else {
            continue;
        };
        if sheet_whole(&entry.path()) {
            state
                .sheets
                .insert(n);
        }
    }
    state
}

/// The sheets a set still needs, lowest first. Once a run has reached the
/// end of the file, every sheet not on disk up to the higher of the highest
/// sheet on disk and the one before the last of `expected`: a run that
/// started at the last sheet and made none reached the end with the sheets
/// before it still to make, and a last sheet may never come. Before that,
/// every sheet of the `expected` that is not on disk, and always the last of
/// them, since the end has not been reached.
pub(super) fn missing(state: &SetState, expected: u32) -> Vec<u32> {
    let last = expected.saturating_sub(1);
    if state.end {
        let top = state
            .sheets
            .last()
            .copied()
            .unwrap_or(0)
            .max(last.saturating_sub(1));
        return (0..=top)
            .filter(|n| {
                !state
                    .sheets
                    .contains(n)
            })
            .collect();
    }
    (0..=last)
        .filter(|n| {
            *n == last
                || !state
                    .sheets
                    .contains(n)
        })
        .collect()
}

/// Whether a set's folder is marked complete.
pub(crate) fn is_done(dir: &Path) -> bool {
    dir.join(DONE_MARKER)
        .exists()
}

/// Deletes sheet `n`, if it exists.
pub(super) fn remove_sheet(dir: &Path, n: u32) {
    let _ = std::fs::remove_file(sheet_file(dir, n));
}

/// Deletes `sheets`, which another version made before it was found not to
/// hold the set's file (see [`TrickplayPasses::run_pass`]).
pub(super) fn drop_on_trial(plan: &PassPlan, dir: &Path, sheets: &[u32]) {
    if sheets.is_empty() {
        return;
    }
    for n in sheets {
        remove_sheet(dir, *n);
    }
    info!(
        set_id = %plan.set_id,
        stream_id = %plan.stream_id,
        sheets = sheets.len(),
        "trick play: another version's read ended short; its pictures are made again from the next"
    );
}

/// Deletes the sheets on trial that another version made in this slot,
/// when its read is stopped before its length was checked. Nothing for the
/// version played.
pub(super) fn drop_stand_in(plan: &PassPlan, dir: &Path, on_trial: &[u32]) {
    if plan
        .other_version
        .is_some()
    {
        drop_on_trial(plan, dir, on_trial);
    }
}

/// Deletes the sheet written last, if it was written at or after `since`,
/// and gives its number.
pub(super) fn drop_newest_since(dir: &Path, since: SystemTime) -> Option<u32> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    let newest = entries
        .flatten()
        .filter_map(|e| {
            let n = e
                .file_name()
                .to_str()
                .and_then(parse_sheet)?;
            let written = e
                .metadata()
                .ok()?
                .modified()
                .ok()?;
            (written >= since).then_some((written, n))
        })
        .max();
    let (_, n) = newest?;
    remove_sheet(dir, n);
    Some(n)
}

/// Clears what a pass cut off by a crash may have left in a set's folder,
/// which still holds `busy`: sheets that are not whole, and the sheet written
/// last since the pass began, which may be partly filled if ffmpeg reached
/// the end of its input as it stopped. When that was the highest sheet, the
/// `end` it may have given goes with it, so the next pass reads the end
/// again. A finished set only loses the marker. The folder's time, which
/// records when the set was last shown, is kept.
pub(super) fn mend_after_crash(dir: &Path) {
    let used = std::fs::metadata(dir).and_then(|m| m.modified());
    let busy = dir.join(BUSY_MARKER);
    if !is_done(dir)
        && let Ok(since) = std::fs::metadata(&busy).and_then(|m| m.modified())
    {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let is_sheet = entry
                    .file_name()
                    .to_str()
                    .and_then(parse_sheet)
                    .is_some();
                if is_sheet && !sheet_whole(&entry.path()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        if let Some(n) = drop_newest_since(dir, since)
            && read_set(dir)
                .sheets
                .last()
                .is_none_or(|highest| *highest < n)
        {
            let _ = std::fs::remove_file(dir.join(END_MARKER));
        }
    }
    let _ = std::fs::remove_file(&busy);
    if let (Ok(used), Ok(file)) = (used, std::fs::File::open(dir)) {
        let _ = file.set_modified(used);
    }
}

/// Removes stale temporary files in one stream's directory and returns the
/// size of what is left.
pub(super) fn sweep_dir(dir: &Path, now: SystemTime) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut size = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let is_tmp = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".tmp"));
        let age = meta
            .modified()
            .ok()
            .and_then(|m| {
                now.duration_since(m)
                    .ok()
            })
            .unwrap_or_default();
        if is_tmp && age > TMP_MAX_AGE {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        size += meta.len();
    }
    size
}

/// Frees disk under `root`: removes temporary files older than 10 minutes,
/// then sets not in `active` unused for longer than `keep_for` (never, when it
/// is `None`), then the least recently used sets not in `active` until the
/// total is at most `cap_bytes`. Only directories named by an id are touched.
pub fn prune(
    root: &Path,
    cap_bytes: u64,
    keep_for: Option<Duration>,
    active: &HashSet<Uuid>,
    now: SystemTime,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut total: u64 = 0;
    let mut candidates: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in entries.flatten() {
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| Uuid::parse_str(name).ok())
        else {
            continue;
        };
        // Read before sweeping: removing a file changes the directory's time.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let used = meta
            .modified()
            .unwrap_or(now);
        let path = entry.path();
        let size = sweep_dir(&path, now);
        if active.contains(&id) {
            total += size;
            continue;
        }
        let age = now
            .duration_since(used)
            .unwrap_or_default();
        if keep_for.is_some_and(|keep| age > keep) {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        total += size;
        candidates.push((path, used, size));
    }
    candidates.sort_by_key(|(_, used, _)| *used);
    for (path, _, size) in candidates {
        if total <= cap_bytes {
            break;
        }
        if std::fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// The prune at server start, run while holding the pass slot (see
/// [`prune_at_startup`]), so no pass runs meanwhile. With the switch off,
/// every set is deleted: only folders named by an id, then `root` itself if
/// nothing else is left in it.
pub fn prune_on_start(
    root: &Path,
    enabled: bool,
    cap_bytes: u64,
    keep_for: Option<Duration>,
    now: SystemTime,
) {
    if !root.exists() {
        return;
    }
    if !enabled {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let named_by_id = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| Uuid::parse_str(name).is_ok());
                let is_dir = entry
                    .file_type()
                    .is_ok_and(|t| t.is_dir());
                if named_by_id && is_dir {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        // Fails, and keeps it, while anything else is in it.
        let _ = std::fs::remove_dir(root);
        return;
    }
    prune(root, cap_bytes, keep_for, &HashSet::new(), now);
    // Only a set a crash left `busy` is checked; any other set, finished or
    // not, holds whole sheets and is left as it is.
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir
            .join(BUSY_MARKER)
            .is_file()
        {
            mend_after_crash(&dir);
        }
    }
}

/// The key check: a run that read to the end of the file and wrote a sheet,
/// `highest`, past the last one its picture count fills read a file longer
/// than its stored length, so its pictures are not the set's. They move to a
/// folder named by the stream itself, which [`set_for`] then keeps that
/// stream on. Gives the folder the pictures are in afterwards.
pub(super) fn moved_if_longer(plan: &PassPlan, dir: &Path, highest: u64) -> PathBuf {
    let longer = highest > last_sheet(plan.pictures);
    if !longer || plan.set_id == plan.stream_id {
        return dir.to_path_buf();
    }
    let own = set_dir(&plan.root, plan.stream_id);
    if own.exists() {
        warn!(
            set_id = %plan.set_id,
            stream_id = %plan.stream_id,
            "trick play: the file is longer than its stored length, but its own folder exists"
        );
        return dir.to_path_buf();
    }
    match std::fs::rename(dir, &own) {
        Ok(()) => {
            info!(
                set_id = %plan.set_id,
                stream_id = %plan.stream_id,
                highest_sheet = highest,
                "trick play: the file is longer than its stored length; its pictures moved to a set of its own"
            );
            own
        }
        Err(e) => {
            warn!(error = %e, "could not move trick play pictures to their own folder");
            dir.to_path_buf()
        }
    }
}
