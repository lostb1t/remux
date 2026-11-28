//! Jellyfin trick play pictures (seek previews), made in the background while
//! a video plays.
//!
//! Off by default (`TrickplayWhileStreaming`). When on, a play-start report
//! queues a low-priority ffmpeg that reads the file through this server's own
//! stream proxy, decodes key frames only and writes one 320x180 picture every
//! 10 seconds, 25 to a sheet, under `{data_dir}/trickplay/{set id}/{n}.jpg`.
//!
//! A pass works in order from the lowest missing sheet, so a set never has a
//! gap. With `TrickplayStartAtPosition` also on (see [`start_at_position`]),
//! it starts at the sheet the viewer is on, reads to the end of the file, then
//! comes back for every sheet still missing, and moves (a few times at most)
//! when the viewer seeks to a part with no pictures yet. The set's
//! folder is the only record: sheets on disk are kept and built on, and the
//! set is `done` only once a run reached the end and no sheet is missing.
//!
//! A set belongs to a title and a picture count (see [`set_id`]), so every
//! version of a title whose measured length gives the same number of pictures
//! shares one set, and a version of another length gets its own. One pass runs
//! at a time, a pass is shared by every play of the same set, it gives way
//! while nobody is watching it and carries on when someone reports again, and
//! it stops a grace period after the last of those plays ends.
//!
//! Who may be read is the addon's choice (see [`AddonRule`]): only streams
//! of an enabled addon with the "seek previews" resource switched on, never
//! IPTV, never an addon that bypasses the proxy while it gives streams, never
//! a deleted addon's leftovers. A stream that names no addon is judged by its
//! row alone. With "stream" also on, the version played may
//! be read, and another version of the same title that is certain to give the
//! same pictures may stand in for it (see [`other_versions`]), into the same
//! set: after the file's own read failed, or at once when its addon is not to
//! be read.
//!
//! An addon with "seek previews" on and "stream" off is kept for seek
//! previews only (see [`previews_only`]). No viewer is offered its streams:
//! after a play has run [`START_DELAY`] and is still watched, while its set is
//! not complete and the version played has a measured length, the pass asks
//! it in the background (see [`PreviewFinder`]), at most once per title and
//! addon in [`PREVIEW_ASKED_WAIT`] ([`PREVIEW_RETRY_WAIT`] after a failed ask),
//! one ask per title at a time, each dropped after [`PREVIEW_ASK_WAIT`]. Its
//! answers are kept apart from the title's versions (see
//! [`db::PreviewSource`]) and read only through this server's own stream
//! proxy. A stream of the same length is read in the addon's own order (see
//! [`preview_versions`]); one with no known length is measured first, at most
//! [`MAX_PREVIEW_MEASURES`] a play.
//!
//! The order of reading in one pass: up to [`MAX_PREVIEW_READS`] streams of
//! addons kept for seek previews only, then the version played if its addon
//! allows, then its other versions. A source that fails is left alone, for
//! [`FAILURE_WAIT`] when it is the version played and [`OTHER_FAILURE_WAIT`]
//! otherwise, and a pass reads at most [`MAX_SOURCES`] sources. When a
//! stand-in's read ends by itself short of the end with no sign that its
//! source failed (a shorter file), the sheets it made are deleted.

mod eligible;
mod ffmpeg;
mod finder;
mod manifest;
mod passes;
mod plan;
mod runs;
mod sets;
mod store;
#[cfg(test)]
mod tests;
mod versions;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use remux_sdks::stremio::ResourceType;
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    AppContext,
    addons::Addon,
    api,
    common::HideConsole,
    db,
    ids::UserId,
    services::StreamService,
    stream::{StreamDescriptor, StreamInfo},
};

pub use eligible::*;
pub use ffmpeg::*;
pub use finder::*;
pub use manifest::*;
pub use passes::*;
pub use plan::*;
use runs::*;
pub use sets::*;
pub use store::*;
pub use versions::*;

/// Seconds between two pictures.
pub const INTERVAL_SECS: u64 = 10;
/// Picture width, also the only width served.
pub const WIDTH: u32 = 320;
/// Picture height.
pub const HEIGHT: u32 = 180;
/// Pictures across one sheet.
pub const TILE_WIDTH: u32 = 5;
/// Pictures down one sheet.
pub const TILE_HEIGHT: u32 = 5;
/// Passes that may run at once.
pub const MAX_PASSES: usize = 1;
/// How long a pass outlives the last play of its file.
pub const GRACE: Duration = Duration::from_secs(60);
/// How long a play runs before its pass may start, so the player's first
/// buffering has the source to itself.
pub const START_DELAY: Duration = Duration::from_secs(15);
/// How often, during the start delay, a version played with no measured
/// length is looked at again for one read from its header as it plays.
pub const LENGTH_CHECK: Duration = Duration::from_secs(2);
/// How long after a failed pass the same file gets no new one.
pub const FAILURE_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long another version that could not be read is not read again.
pub const OTHER_FAILURE_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// Sources one pass reads at most: preview sources, the file played and
/// other versions.
pub const MAX_SOURCES: usize = 3;
/// Streams of addons kept for seek previews only that one pass reads at most,
/// before the version played.
pub const MAX_PREVIEW_READS: usize = 2;
/// Streams of one answer of an addon kept for seek previews only that are
/// kept, the first in its order that pass the checks. A choice; nothing was
/// measured for it.
pub const MAX_PREVIEW_KEPT: usize = 5;
/// Kept streams with no known length that one play measures at most, one
/// short read each.
pub const MAX_PREVIEW_MEASURES: usize = 2;
/// How long one ask of an addon kept for seek previews only may take before
/// it is dropped.
pub const PREVIEW_ASK_WAIT: Duration = Duration::from_secs(30);
/// How long after an answer the same addon is not asked again for the same
/// title. Its answer is kept as long (see [`db::PREVIEW_SOURCE_TTL`]).
pub const PREVIEW_ASKED_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after a failed or dropped ask the same addon is not asked again
/// for the same title.
pub const PREVIEW_RETRY_WAIT: Duration = Duration::from_secs(60 * 60);
/// How long the size of another version is waited for before it is read.
pub const SIZE_WAIT: Duration = Duration::from_secs(30);
/// How often a pass checks that someone is still watching it.
pub const WATCH_CHECK: Duration = Duration::from_secs(15);
/// A pass whose plays have all been silent this long gives way to one that
/// is waiting for the slot.
pub const QUIET_WAITING: Duration = Duration::from_secs(60);
/// A pass whose plays have all been silent this long stops even with nobody
/// waiting. It carries on when one of them reports again.
pub const QUIET_ALONE: Duration = Duration::from_secs(5 * 60);
/// Days pictures are kept after they were last shown, unless set otherwise.
/// The admin page quotes this value.
pub const DEFAULT_KEEP_DAYS: i64 = 30;
/// Longest keep time that can be set, in days.
pub const MAX_KEEP_DAYS: i64 = 3_650;
/// Shortest runtime that gets pictures. The admin page quotes this value.
pub const MIN_RUNTIME_SECS: u64 = 300;
/// Longest runtime that gets pictures. The admin page quotes this value.
pub const MAX_RUNTIME_SECS: u64 = 4 * 60 * 60;
/// No pass starts, and a running one stops, below this much free disk.
pub const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The shortest wait for the first sheet of a run that starts part way into
/// the file. A source that gives none in time is read from its start.
pub const SEEK_WAIT: Duration = Duration::from_secs(120);
/// How long a source that gave no sheet in time after a seek (see
/// [`SEEK_WAIT`]) is read from its start. One slow moment then does not
/// mark it until the server restarts.
pub const NO_JUMP_WAIT: Duration = Duration::from_secs(60 * 60);
/// A run that makes no new sheet for this long has stalled: it is stopped
/// and its stream counts as failed, so it does not hold the one slot for as
/// long as someone watches. One sheet covers 250 s of video, so this leaves
/// room for a source read at little more than playing speed.
pub const STALL_WAIT: Duration = Duration::from_secs(10 * 60);
/// A pass follows the viewer to a part with no pictures at most this often,
/// with [`start_at_position`] only.
pub const FOLLOW_EVERY: Duration = Duration::from_secs(30);
/// A pass follows the viewer at most this many times, with
/// [`start_at_position`] only. The admin page
/// quotes this value.
pub const MAX_FOLLOWS: u32 = 4;
/// Directory under `data_dir` that holds every set of pictures.
pub const ROOT_DIR: &str = "trickplay";

/// Seconds of video one sheet covers.
const SHEET_SECS: u64 = INTERVAL_SECS * (TILE_WIDTH * TILE_HEIGHT) as u64;
/// Estimated bits per second of pictures (about 13 KB a picture), as
/// Jellyfin's `Bandwidth`. Clients ignore it.
const BANDWIDTH: i32 = 10_400;
/// Written into a set's directory once it is complete.
const DONE_MARKER: &str = "done";
/// Written into a set's directory once a run has reached the end of the file.
const END_MARKER: &str = "end";
/// Present in a set's directory while a pass is at work there; one left by a
/// crash tells the start-up prune to check the sheets.
const BUSY_MARKER: &str = "busy";
/// Runs one pass makes at most: two (from the viewer to the end, then the
/// beginning), two for each move to the viewer with [`start_at_position`]
/// (the run it starts, and one more for the stretch it splits), one after a
/// run that found nothing and one after a run that could not jump.
const MAX_RUNS: u32 = 2 * MAX_FOLLOWS + 4;
/// The slowest read assumed when waiting for the first sheet of a run that
/// starts part way: the wait covers one sheet's share of the file at this
/// rate, in bytes a second.
const SEEK_READ_BYTES_PER_SEC: u64 = 5 * 1024 * 1024;
/// Temporary files of an unfinished write older than this are deleted.
const TMP_MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// A served directory's use time is updated at most this often.
const TOUCH_EVERY: Duration = Duration::from_secs(60);
/// Smallest disk allowance, in MB. The admin page quotes this value.
const MIN_CACHE_MB: i64 = 100;
/// How long `kill_all` waits for each pass before killing its process. The
/// kill after it works on Linux and macOS only (see [`kill_pid`]).
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How much of ffmpeg's stderr a run keeps for its log, besides the lines
/// that judge it (see [`StderrKept`]).
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// The longest stderr line kept; the rest of a longer one is left out.
const STDERR_LINE_BYTES: usize = 1024;
const TICKS_PER_SEC: i64 = 10_000_000;

/// Why a stream gets no pictures.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display, strum_macros::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum Ineligible {
    /// No stream row, or one with no stream information.
    NoStream,
    LiveTv,
    Rtsp,
    Catchup,
    Torrent,
    Hls,
    NoRuntime,
    TooShort,
    TooLong,
    Iptv,
    Redirect,
    /// The addon's "seek previews" resource is off, the addon is disabled, or
    /// it was deleted.
    AddonExcluded,
    /// The addon the stream names could not be read, so its switches are not
    /// known.
    AddonReadFailed,
    /// The item played is not a movie, an episode or a stream: music, for one.
    NotVideo,
}

/// Whether the switch is on.
pub fn enabled(cfg: &api::ServerConfiguration) -> bool {
    cfg.trickplay_while_streaming
        .unwrap_or(false)
}

/// Whether a pass starts at the position being watched and follows seeks:
/// both switches on. Otherwise it works in order from the lowest missing
/// sheet, so a set never has a gap.
pub fn start_at_position(cfg: &api::ServerConfiguration) -> bool {
    enabled(cfg)
        && cfg
            .trickplay_start_at_position
            .unwrap_or(false)
}

/// Whether a list asked for with `fields` gets the `Trickplay` field: it
/// names the field and the switch is on. The settings are read only then.
pub async fn wanted_in_list(
    ctx: &AppContext,
    fields: Option<&[api::ItemFields]>,
) -> bool {
    fields.is_some_and(|f| f.contains(&api::ItemFields::Trickplay))
        && enabled(&db::Settings::get_config_or_default(&ctx.db).await)
}

/// The disk allowance for pictures, in bytes.
pub fn cap_bytes(cfg: &api::ServerConfiguration) -> u64 {
    let mb = cfg
        .trickplay_cache_size_mb
        .unwrap_or(1024)
        .max(MIN_CACHE_MB);
    u64::try_from(mb)
        .unwrap_or(0)
        .saturating_mul(1024 * 1024)
}

/// How long a set is kept after it was last shown: `None` never deletes by
/// age (0, or a negative value), otherwise at most 3,650 days.
pub fn keep_for(cfg: &api::ServerConfiguration) -> Option<Duration> {
    let days = cfg
        .trickplay_keep_days
        .unwrap_or(DEFAULT_KEEP_DAYS)
        .min(MAX_KEEP_DAYS);
    let days = u64::try_from(days)
        .ok()
        .filter(|d| *d > 0)?;
    Some(Duration::from_secs(days * 24 * 60 * 60))
}
