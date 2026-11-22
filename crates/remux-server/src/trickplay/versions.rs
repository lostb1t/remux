//! Other versions of a title that may be read in place of the one played.

use super::*;

/// How another version of a title is held, cheapest to read first: a file on
/// this server's disk, one on a library addon's WebDAV server, then one a
/// named service marks as cached, outside usenet before usenet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Held {
    Local,
    Opendal,
    Cached,
    CachedUsenet,
}

/// How a stream is held, or `None` when it may not stand in for another: a
/// plain link with no named service (as IPTV on demand is), one not marked
/// cached, or a torrent. A cached mark with no service is not enough.
pub(super) fn held(info: &StreamInfo) -> Option<Held> {
    match info.descriptor {
        StreamDescriptor::Local(_) => return Some(Held::Local),
        StreamDescriptor::Opendal { .. } => return Some(Held::Opendal),
        StreamDescriptor::Torrent { .. } | StreamDescriptor::Rtsp { .. } => {
            return None;
        }
        StreamDescriptor::Http { .. } => {}
    }
    let cached = info
        .service_id
        .as_deref()
        .is_some_and(|s| !s.is_empty())
        && info.service_cached == Some(true);
    if !cached {
        return None;
    }
    let usenet = info
        .usenet_guid
        .is_some()
        || info
            .nzb_url
            .is_some();
    Some(if usenet {
        Held::CachedUsenet
    } else {
        Held::Cached
    })
}

/// The checks a stream row passes to be read in place of the version played,
/// all but its length: it is a stream row with no folder of its own, it
/// passes the row checks, its addon is in `rules` and allows it, a plain link
/// names a service that marks it cached (a cached mark alone is what IPTV on
/// demand lists), a service's file lists its size, and its stored length is
/// of the file it lists. Gives how it is held.
pub(super) fn stand_in(
    row: &db::Media,
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: &impl Fn(Uuid) -> bool,
) -> Option<Held> {
    if row.kind != db::MediaKind::Stream || has_own_folder(row.id) {
        return None;
    }
    let info = row
        .stream_info
        .as_ref()?;
    eligible_row(row, info, runtime_s(row, item_runtime_s)).ok()?;
    let allowed = info
        .addon_id
        .and_then(|id| rules.get(&id))
        .is_some_and(AddonRule::allows);
    if !allowed {
        return None;
    }
    let held = held(info)?;
    let size = info
        .size
        .filter(|s| *s > 0);
    if held >= Held::Cached && size.is_none() {
        return None;
    }
    let probed_size = row
        .probe_data
        .as_ref()
        .and_then(|p| p.size);
    if info.is_other_file(probed_size) {
        return None;
    }
    Some(held)
}

/// The other versions of `playing`'s title that may be read in its place:
/// a file on this server's disk, then one on a library addon's WebDAV
/// server, then one a named service marks as cached (outside usenet before
/// usenet), then the smallest listed file, then the listing order. Each must
/// give the same pictures at the same places: its own measured length gives
/// `playing`'s picture count (so none when `playing` has no measured length),
/// and it passes the checks of [`stand_in`]. `versions` are the title's
/// stream rows.
pub fn other_versions<'a>(
    playing: &db::Media,
    versions: &'a [db::Media],
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: impl Fn(Uuid) -> bool,
) -> Vec<&'a db::Media> {
    let set = set_id(playing);
    if set == playing.id {
        return Vec::new();
    }
    let mut found: Vec<(Held, i64, i64, &db::Media)> = versions
        .iter()
        .filter(|row| row.id != playing.id && set_id(row) == set)
        .filter_map(|row| {
            let held = stand_in(row, item_runtime_s, rules, &has_own_folder)?;
            let size = row
                .stream_info
                .as_ref()
                .and_then(|info| info.size)
                .filter(|s| *s > 0);
            Some((
                held,
                size.unwrap_or(i64::MAX),
                row.idx
                    .unwrap_or(i64::MAX),
                row,
            ))
        })
        .collect();
    found.sort_by_key(|(held, size, idx, _)| (*held, *size, *idx));
    found
        .into_iter()
        .map(|(_, _, _, row)| row)
        .collect()
}

/// The streams of `rows` that would stand in for `playing` (see
/// [`stand_in`]) but for their length: measured to another picture count.
/// One with no measured length is not counted, since its length is not
/// known. None when `playing` has no measured length. Counted for the log
/// when a play gets no pictures for want of a file of its length.
pub fn other_lengths(
    playing: &db::Media,
    rows: &[db::Media],
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: impl Fn(Uuid) -> bool,
) -> usize {
    let set = set_id(playing);
    if set == playing.id {
        return 0;
    }
    rows.iter()
        .filter(|row| {
            row.id != playing.id && measured_ticks(row).is_some() && set_id(row) != set
        })
        .filter(|row| stand_in(row, item_runtime_s, rules, &has_own_folder).is_some())
        .count()
}

/// The stored streams of addons kept for seek previews only that may be read
/// for `playing`'s pictures. `rows` are the title's stored answers, each as a
/// stream row under the title (see [`db::PreviewSource::to_stream_row`]).
/// Each passes the checks of [`other_versions`], and its addon is still one
/// kept for seek previews only. In the addon order, then in each addon's own
/// order, not by size: an admin sorts a dedicated addon's results by quickest
/// to load.
pub fn preview_versions<'a>(
    playing: &db::Media,
    rows: &'a [db::Media],
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: impl Fn(Uuid) -> bool,
) -> Vec<&'a db::Media> {
    let set = set_id(playing);
    if set == playing.id {
        return Vec::new();
    }
    in_addon_order(
        rows.iter()
            .filter(|row| row.id != playing.id && set_id(row) == set),
        item_runtime_s,
        rules,
        &has_own_folder,
    )
}

/// The rows of `rows` that pass every check of [`preview_versions`] but have
/// no measured length yet, in the same order: the ones one short read could
/// make fit (see [`MAX_PREVIEW_MEASURES`]).
pub fn preview_unmeasured<'a>(
    rows: &'a [db::Media],
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: impl Fn(Uuid) -> bool,
) -> Vec<&'a db::Media> {
    in_addon_order(
        rows.iter()
            .filter(|row| measured_ticks(row).is_none()),
        item_runtime_s,
        rules,
        &has_own_folder,
    )
}

/// The rows that pass [`stand_in`] and whose addon is kept for seek previews
/// only, by addon order, then by their place in the addon's answer.
pub(super) fn in_addon_order<'a>(
    rows: impl Iterator<Item = &'a db::Media>,
    item_runtime_s: Option<i64>,
    rules: &AddonRules,
    has_own_folder: &impl Fn(Uuid) -> bool,
) -> Vec<&'a db::Media> {
    let mut found: Vec<(i64, Uuid, i64, &db::Media)> = rows
        .filter_map(|row| {
            stand_in(row, item_runtime_s, rules, has_own_folder)?;
            let addon_id = row
                .stream_info
                .as_ref()?
                .addon_id?;
            let rule = rules
                .get(&addon_id)
                .filter(|rule| rule.preview_only)?;
            Some((
                rule.priority,
                addon_id,
                row.idx
                    .unwrap_or(i64::MAX),
                row,
            ))
        })
        .collect();
    found.sort_by_key(|(priority, addon_id, idx, _)| (*priority, *addon_id, *idx));
    found
        .into_iter()
        .map(|(_, _, _, row)| row)
        .collect()
}

/// Whether `found`, the size in bytes another version's source gave just
/// before it is read, is that of the file it lists (see
/// [`StreamInfo::is_other_file`]). A source that gave none is not read.
pub(super) fn is_listed_file(info: &StreamInfo, found: Option<u64>) -> bool {
    let found = found
        .and_then(|s| i64::try_from(s).ok())
        .filter(|s| *s > 0);
    found.is_some() && !info.is_other_file(found)
}
