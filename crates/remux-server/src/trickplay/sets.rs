//! Which set of pictures a stream belongs to, and where a set lives on disk.

use super::*;

/// `{data_dir}/trickplay`.
pub fn root_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(ROOT_DIR)
}

/// The directory holding one set's sheets.
pub fn set_dir(root: &Path, set_id: Uuid) -> PathBuf {
    root.join(
        set_id
            .as_simple()
            .to_string(),
    )
}

/// Path of sheet `n` of a set.
pub fn sheet_path(root: &Path, set_id: Uuid, n: u32) -> PathBuf {
    sheet_file(&set_dir(root, set_id), n)
}

/// Path of sheet `n` in a set's directory.
pub(super) fn sheet_file(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("{n}.jpg"))
}

/// The runtime used to judge a stream, in seconds: the probed runtime, then
/// the item's own runtime, then the duration the source listed.
pub fn runtime_s(stream: &db::Media, item_runtime_s: Option<i64>) -> Option<u64> {
    let probed = stream
        .probe_data
        .as_ref()
        .and_then(|p| p.run_time_ticks)
        .map(|ticks| ticks / TICKS_PER_SEC);
    let listed = stream
        .stream_info
        .as_ref()
        .and_then(|info| info.duration);
    probed
        .filter(|s| *s > 0)
        .or(item_runtime_s.filter(|s| *s > 0))
        .or(listed.filter(|s| *s > 0))
        .and_then(|s| u64::try_from(s).ok())
}

/// Pictures a length in ticks gives: one every 10 seconds, rounded up.
/// [`manifest`], [`set_id`] and a pass's own count all use it, so a client
/// is told the count its set is keyed by.
pub fn picture_count(ticks: i64) -> u64 {
    let interval_ticks = INTERVAL_SECS as i64 * TICKS_PER_SEC;
    u64::try_from(ticks.max(0))
        .unwrap_or(0)
        .div_ceil(interval_ticks as u64)
}

/// A stream's own measured length in ticks: probed here, taken from
/// RemuxDB, or read from the file's header as it was relayed (see
/// [`store_length`]). A length guessed from the file name, or the title's
/// listed runtime stored in its place, is not one.
pub fn measured_ticks(stream: &db::Media) -> Option<i64> {
    stream
        .probe_data
        .as_ref()
        .and_then(probe_ticks)
}

pub(super) fn probe_ticks(probe: &api::MediaSourceInfo) -> Option<i64> {
    let measured = matches!(
        probe
            .remux
            .as_ref()
            .and_then(|r| r.source),
        Some(
            api::ProbeOrigin::Ffprobe
                | api::ProbeOrigin::RemuxDb
                | api::ProbeOrigin::Container
        )
    );
    probe
        .run_time_ticks
        .filter(|t| measured && *t > 0)
}

/// Whether the bytes relayed for `stream` are read for the length its file
/// states: a stream row under a title with no measured length.
pub fn wants_length(stream: &db::Media) -> bool {
    stream.kind == db::MediaKind::Stream
        && stream
            .parent_id
            .is_some()
        && measured_ticks(stream).is_none()
}

/// `probe` with `ticks` as the length read from the file's header and every
/// other fact kept, or `None` when it has a measured length already.
pub(super) fn with_length(
    probe: Option<api::MediaSourceInfo>,
    ticks: i64,
) -> Option<api::MediaSourceInfo> {
    let mut probe = probe.unwrap_or_default();
    if probe_ticks(&probe).is_some() {
        return None;
    }
    probe.run_time_ticks = Some(ticks);
    probe
        .remux
        .get_or_insert_with(Default::default)
        .source = Some(api::ProbeOrigin::Container);
    Some(probe)
}

/// Stores the length a stream's file states, read from its header as it was
/// relayed, once: a row measured meanwhile keeps its own (see
/// [`db::Media::change_probe_data`]).
pub async fn store_length(
    ctx: AppContext,
    stream_id: Uuid,
    title: String,
    ticks: i64,
    container: &'static str,
) {
    match db::Media::change_probe_data(&ctx.db, &stream_id, |probe| {
        with_length(probe, ticks)
    })
    .await
    {
        Ok(true) => debug!(
            %stream_id,
            %title,
            seconds = ticks as f64 / TICKS_PER_SEC as f64,
            container,
            "trick play: a stream's length was read from its header"
        ),
        Ok(false) => {}
        Err(error) => {
            warn!(%stream_id, %error, "trick play: could not store a stream's length")
        }
    }
}

/// The set a stream's pictures belong to. Versions of one title share a set
/// when their measured lengths give the same picture count, so each is told
/// the same count and the same sheet numbers hold the same moments. Anything
/// else (not measured, no title, not a stream row) keeps a set of its own,
/// named by its own id.
pub fn set_id(stream: &db::Media) -> Uuid {
    match (stream.kind == db::MediaKind::Stream, stream.parent_id) {
        (true, Some(parent)) => measured_ticks(stream).map_or(stream.id, |ticks| {
            Uuid::new_v5(
                &parent,
                format!("trickplay:{}", picture_count(ticks)).as_bytes(),
            )
        }),
        _ => stream.id,
    }
}

/// [`set_id`], except that a stream whose own folder exists keeps it: a set
/// made before the stream was measured, or one moved there because the
/// stream turned out longer than its stored length.
pub fn set_for(root: &Path, stream: &db::Media) -> Uuid {
    if set_dir(root, stream.id).is_dir() {
        stream.id
    } else {
        set_id(stream)
    }
}

/// The set a stored stream's pictures belong to (see [`set_for`]); the id
/// itself when no stream row has it. With it, the picture count the set is
/// keyed by, when it is keyed by one. One row read by primary key.
pub async fn set_for_stream(ctx: &AppContext, stream_id: Uuid) -> (Uuid, Option<u64>) {
    let root = root_dir(
        &ctx.config
            .data_dir,
    );
    let Some(stream) = stored_stream(ctx, stream_id).await else {
        return (stream_id, None);
    };
    let set_id = set_for(&root, &stream);
    let pictures = measured_ticks(&stream)
        .filter(|_| set_id != stream.id)
        .map(picture_count);
    (set_id, pictures)
}

/// The stream row with this id, if it is one.
pub(super) async fn stored_stream(ctx: &AppContext, id: Uuid) -> Option<db::Media> {
    db::Media::get_by_id(&ctx.db, &id)
        .await
        .ok()
        .flatten()
        .filter(|m| m.kind == db::MediaKind::Stream)
}
