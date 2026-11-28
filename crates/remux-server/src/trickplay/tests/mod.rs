//! Tests of background trick play, and the fixtures they share.

mod ffmpeg;
mod passes;
mod preview_sources;
mod rules;
mod stand_ins;
mod store;

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

const RUNTIME: u64 = 7_200;
const URL: &str = "http://example.invalid/a.mkv";

fn fast() -> Limits {
    Limits {
        grace: Duration::from_millis(50),
        start_delay: Duration::from_millis(10),
        length_check: Duration::from_millis(5),
        failure_wait: FAILURE_WAIT,
        other_failure_wait: OTHER_FAILURE_WAIT,
        min_free_bytes: 0,
        watch_check: Duration::from_millis(10),
        quiet_waiting: Duration::from_millis(100),
        quiet_alone: Duration::from_secs(10),
        seek_wait: Duration::from_secs(1),
        no_jump_wait: NO_JUMP_WAIT,
        stall_wait: Duration::from_secs(60),
        follow_every: Duration::from_millis(30),
        max_follows: MAX_FOLLOWS,
    }
}

/// A pass over a 7,200 s stream with a set of its own.
fn plan(root: &Path, stream_id: Uuid) -> PassPlan {
    set_plan(root, stream_id, stream_id)
}

/// A pass over a 7,200 s stream (720 pictures) into `set_id`.
fn set_plan(root: &Path, stream_id: Uuid, set_id: Uuid) -> PassPlan {
    PassPlan {
        stream_id,
        set_id,
        input: "/media/invented-film.mkv".to_string(),
        root: root.to_path_buf(),
        pictures: RUNTIME / INTERVAL_SECS,
        size: None,
        estimate: 0,
        played: StreamInfo::default(),
        // Today's tests start at the viewer; the tests of the default
        // set it off.
        from_position: true,
        db: None,
        others: Vec::new(),
        other_version: None,
        preview: None,
        played_refused: None,
        other_lengths: 0,
        delay_spent: Duration::ZERO,
    }
}

/// A play that reported just now, whatever the device and item.
fn always() -> impl Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static {
    |_: &str, _: Uuid| {
        Some(Watched {
            quiet: Duration::ZERO,
            position_s: 0,
            play_session_id: String::new(),
        })
    }
}

/// No device has a reported session on the item.
fn never() -> impl Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static {
    |_: &str, _: Uuid| None
}

/// What one device last reported: when (`None` is "just now", whenever
/// asked), for which play session, and at which position.
#[derive(Clone, Default)]
struct Report {
    last: Option<Instant>,
    play_session_id: String,
    position_s: u64,
}

/// Each device's last report. A device not listed has no reported
/// session on the item.
#[derive(Clone, Default)]
struct Reports(Arc<Mutex<HashMap<String, Report>>>);

impl Reports {
    fn set(&self, device_id: &str, last: Option<Instant>) {
        self.0
            .lock()
            .unwrap()
            .entry(device_id.to_string())
            .or_default()
            .last = last;
    }

    /// Reporting all the time.
    fn fresh(&self, device_id: &str) {
        self.set(device_id, None);
    }

    /// Reporting all the time, for `play_session_id`.
    fn playing(&self, device_id: &str, play_session_id: &str) {
        let mut reports = self
            .0
            .lock()
            .unwrap();
        let report = reports
            .entry(device_id.to_string())
            .or_default();
        report.last = None;
        report.play_session_id = play_session_id.to_string();
    }

    /// Reporting all the time, at `position_s`.
    fn at(&self, device_id: &str, position_s: u64) {
        let mut reports = self
            .0
            .lock()
            .unwrap();
        let report = reports
            .entry(device_id.to_string())
            .or_default();
        report.last = None;
        report.position_s = position_s;
    }

    /// Silent since `ago` ago.
    fn silent(&self, device_id: &str, ago: Duration) {
        self.set(device_id, Instant::now().checked_sub(ago));
    }

    fn watch(&self) -> impl Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static {
        let reports = self.clone();
        move |device_id: &str, _: Uuid| {
            reports
                .0
                .lock()
                .unwrap()
                .get(device_id)
                .map(|report| Watched {
                    quiet: report
                        .last
                        .map_or(Duration::ZERO, |at| at.elapsed()),
                    position_s: report.position_s,
                    play_session_id: report
                        .play_session_id
                        .clone(),
                })
        }
    }
}

/// A sheet as [`sheet_whole`] sees one: JPEG start and end markers.
const JPEG: &[u8] = &[0xFF, 0xD8, b'j', b'p', b'g', 0xFF, 0xD9];

/// The size, in bytes, the other versions in these tests list.
const LISTED: u64 = 6 * 1024 * 1024 * 1024;

fn write_sheets(dir: &Path, sheets: impl IntoIterator<Item = u32>) {
    std::fs::create_dir_all(dir).unwrap();
    for n in sheets {
        std::fs::write(dir.join(format!("{n}.jpg")), JPEG).unwrap();
    }
}

fn sheets_in(dir: &Path) -> Vec<u32> {
    let mut found: Vec<u32> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(parse_sheet)
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_unstable();
    found
}

/// A file of `sheets` sheets. A run makes them from its start sheet. It
/// stops once it has made `stop_after` (its goal), and ends by itself with
/// exit 0 after `per_run` sheets (a source cut off part way) or at the end
/// of the file. On reaching `stall_at` the first time, or instead of
/// ending when `wait_for_cancel`, it waits to be cancelled. With
/// `no_jump`, a run that starts part way makes no first sheet in time.
/// With `empty_exit`, a run that ends by itself having made no sheet
/// exits with that status and writes that to stderr, as the ffmpeg in
/// this server's image does (see [`PAST_END`]); without it, exit 0, as an
/// ffmpeg that ends such a run cleanly would. Each sheet
/// takes `pace`.
///
/// A run of an input in `dead` makes nothing and exits with an error, as
/// a link that answers 404 does. An input in `empty` makes nothing and
/// exits 0, as a source that ends every request at once does. An input in
/// `long` holds one sheet more than `sheets`, one in `short` half of them;
/// one in `cut` is cut off half way, exiting 0 with [`SOURCE_CUT`]. Asked
/// its size, an input answers [`LISTED`], or three times that when it is
/// in `wrong_size`.
#[derive(Default)]
struct FakeRunner {
    sheets: u64,
    per_run: Option<u64>,
    stall_at: Option<u64>,
    wait_for_cancel: bool,
    no_jump: bool,
    empty_exit: Option<(i32, &'static str)>,
    pace: Duration,
    runs: AtomicUsize,
    killed: AtomicBool,
    /// Some run has stalled at `stall_at`; later runs pass it.
    stalled_once: AtomicBool,
    starts: Mutex<Vec<u32>>,
    dead: Vec<String>,
    empty: Vec<String>,
    long: Vec<String>,
    short: Vec<String>,
    cut: Vec<String>,
    wrong_size: Vec<String>,
    /// The input of each run so far.
    inputs: Mutex<Vec<String>>,
    /// The inputs whose size was asked so far.
    sized: Mutex<Vec<String>>,
}

impl FakeRunner {
    /// The start sheet of each run so far.
    fn starts(&self) -> Vec<u32> {
        self.starts
            .lock()
            .unwrap()
            .clone()
    }

    /// The input of each run so far.
    fn inputs(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap()
            .clone()
    }

    /// The inputs whose size was asked so far.
    fn sized(&self) -> Vec<String> {
        self.sized
            .lock()
            .unwrap()
            .clone()
    }
}

impl PassRunner for FakeRunner {
    fn run(
        &self,
        job: PassJob,
        cancel: oneshot::Receiver<()>,
    ) -> impl Future<Output = RunResult> + Send {
        self.runs
            .fetch_add(1, Ordering::SeqCst);
        self.starts
            .lock()
            .unwrap()
            .push(job.start_sheet);
        self.inputs
            .lock()
            .unwrap()
            .push(
                job.input
                    .clone(),
            );
        let dead = self
            .dead
            .contains(&job.input);
        let empty = self
            .empty
            .contains(&job.input);
        let cut = self
            .cut
            .contains(&job.input);
        let sheets = if self
            .long
            .contains(&job.input)
        {
            self.sheets + 1
        } else if cut
            || self
                .short
                .contains(&job.input)
        {
            self.sheets / 2
        } else {
            self.sheets
        };
        async move {
            let mut cancel = cancel;
            if dead {
                return RunResult {
                    exit_code: Some(1),
                    stderr: "[http @ 0x6] HTTP error 404 Not Found\n\
                             Error opening input: Server returned 404 Not Found\n"
                        .to_string(),
                    ..Default::default()
                };
            }
            if empty {
                return RunResult {
                    exit_ok: true,
                    exit_code: Some(0),
                    ..Default::default()
                };
            }
            let start = u64::from(job.start_sheet);
            if self.no_jump && start > 0 {
                return RunResult {
                    no_first_sheet: true,
                    ..Default::default()
                };
            }
            let mut made = 0;
            let mut stalled = false;
            while start + made < sheets
                && self
                    .per_run
                    .is_none_or(|most| made < most)
            {
                if job
                    .stop_after
                    .is_some_and(|goal| made >= goal)
                {
                    return RunResult {
                        made_sheets: made,
                        reached_goal: true,
                        ..Default::default()
                    };
                }
                if self.stall_at == Some(start + made)
                    && !self
                        .stalled_once
                        .swap(true, Ordering::SeqCst)
                {
                    stalled = true;
                    break;
                }
                std::fs::write(
                    job.out_dir
                        .join(format!("{}.jpg", start + made)),
                    JPEG,
                )
                .unwrap();
                made += 1;
                job.made
                    .store(u32::try_from(made).unwrap(), Ordering::SeqCst);
                if !self
                    .pace
                    .is_zero()
                {
                    tokio::select! {
                        _ = &mut cancel => {
                            self.killed
                                .store(true, Ordering::SeqCst);
                            return RunResult {
                                made_sheets: made,
                                cancelled: true,
                                ..Default::default()
                            };
                        }
                        _ = tokio::time::sleep(self.pace) => {}
                    }
                }
            }
            if stalled || self.wait_for_cancel {
                let _ = cancel.await;
                self.killed
                    .store(true, Ordering::SeqCst);
                return RunResult {
                    made_sheets: made,
                    cancelled: true,
                    ..Default::default()
                };
            }
            if made == 0
                && let Some((code, stderr)) = self.empty_exit
            {
                return RunResult {
                    exit_ok: code == 0,
                    exit_code: Some(code),
                    stderr: stderr.to_string(),
                    ..Default::default()
                };
            }
            RunResult {
                exit_ok: true,
                exit_code: Some(0),
                made_sheets: made,
                stderr: if cut {
                    SOURCE_CUT.to_string()
                } else {
                    String::new()
                },
                ..Default::default()
            }
        }
    }

    fn size(&self, input: &str) -> impl Future<Output = Option<u64>> + Send {
        self.sized
            .lock()
            .unwrap()
            .push(input.to_string());
        let size = if self
            .wrong_size
            .iter()
            .any(|i| i == input)
        {
            LISTED * 3
        } else {
            LISTED
        };
        async move { Some(size) }
    }
}

/// What the ffmpeg in this server's image wrote at `-loglevel error` for
/// a run with the pass's arguments started past the end of a file, exit
/// 234 and no file written (the end-to-end pre-check, from the file
/// itself).
const PAST_END: &str = "\
    [mjpeg @ 0x8] Non full-range YUV is non-standard, set strict_std_compliance to at \
    most unofficial to use it.\n\
    [mjpeg @ 0x1] ff_frame_thread_encoder_init failed\n\
    [vost#0:0/mjpeg @ 0x2] [enc:mjpeg @ 0x3] Error while opening encoder - maybe incorrect \
    parameters such as bit_rate, rate, width or height.\n\
    [vf#0:0 @ 0x4] Error sending frames to consumers: Invalid argument\n\
    [vf#0:0 @ 0x4] Task finished with error code: -22 (Invalid argument)\n\
    [vf#0:0 @ 0x4] Terminating thread with return code -22 (Invalid argument)\n\
    [vost#0:0/mjpeg @ 0x2] [enc:mjpeg @ 0x3] Could not open encoder before EOF\n\
    [vost#0:0/mjpeg @ 0x2] Task finished with error code: -22 (Invalid argument)\n\
    [vost#0:0/mjpeg @ 0x2] Terminating thread with return code -22 (Invalid argument)\n\
    [out#0/image2 @ 0x5] Nothing was written into output file, because at least one of \
    its streams received no packets.\n";

/// The same over HTTP from a source that closed a request at the file's
/// very end (the pre-check over HTTP), before the lines of [`PAST_END`].
const PAST_END_HTTP: &str = "\
    [http @ 0x6] Error reading HTTP response: End of file\n\
    [in#0/matroska,webm @ 0x7] File ended prematurely at pos. 70027876 (0x42c8a64)\n";

/// A source that dropped after the seek and did not come back, before the
/// lines of [`PAST_END`]: ffmpeg then ends its input as at the end of
/// the file, so the status is the same.
const SOURCE_DROPPED: &str = "\
    [http @ 0x6] Stream ends prematurely at 52428800, should be 70027876\n\
    [in#0/matroska,webm @ 0x7] Read error at pos. 52428800 (0x3200000)\n\
    [in#0/matroska,webm @ 0x7] Error during demuxing: Input/output error\n";

/// What this server's ffmpeg wrote, with exit 0 and sheets made, for a
/// source that cut every answer off part way (the end-to-end check).
const SOURCE_CUT: &str = "\
    [http @ 0x6] Stream ends prematurely at 49019513, should be 70027876\n\
    [http @ 0x6] Error reading HTTP response: End of file\n\
    [http @ 0x6] Stream ends prematurely at 49019513, should be 18446744073709551615\n\
    [in#0/matroska,webm @ 0x7] Read error\n\
    [http @ 0x6] Stream ends prematurely at 49019513, should be 70027876\n\
    [in#0/matroska,webm @ 0x8] Error during demuxing: Input/output error\n";

async fn until(check: impl FnMut() -> bool) {
    until_named("condition", check).await;
}

/// As [`until`], naming the wait that gave up.
async fn until_named(label: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{label} not reached");
}

fn row(descriptor: StreamDescriptor) -> (db::Media, StreamInfo) {
    let info = StreamInfo {
        descriptor,
        ..Default::default()
    };
    let media = db::Media {
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Stream,
        stream_info: Some(info.clone()),
        ..Default::default()
    };
    (media, info)
}

/// An addon that gives streams and lets them be read for pictures.
fn addon(kind: &str, redirect: bool) -> Addon {
    let now = chrono::Utc::now().naive_utc();
    Addon {
        id: Uuid::new_v4(),
        name: "Invented Addon".to_string(),
        preset: crate::addons::AddonPresetRef {
            kind: kind.to_string(),
            ..Default::default()
        },
        resources: vec![ResourceType::Stream, ResourceType::SeekPreviews],
        types: None,
        enabled: true,
        priority: 0,
        created_at: now,
        updated_at: now,
        system: false,
        is_default: true,
        http_redirect_stream: redirect,
        subtitle_extraction: false,
        probe_on_scan: false,
        service_filter: vec![],
    }
}

/// An addon kept for seek previews only: "seek previews" on, "stream"
/// off.
fn preview_addon() -> Addon {
    Addon {
        resources: vec![ResourceType::SeekPreviews],
        ..addon("stremio", false)
    }
}

/// `addon` with "seek previews" switched off.
fn switched_off(addon: &Addon) -> Addon {
    Addon {
        resources: addon
            .resources
            .iter()
            .filter(|r| **r != ResourceType::SeekPreviews)
            .cloned()
            .collect(),
        ..addon.clone()
    }
}

fn item(
    type_: api::MediaType,
    runtime_s: Option<i64>,
    ids: &[Uuid],
) -> api::BaseItemDto {
    api::BaseItemDto {
        id: ids
            .first()
            .copied()
            .unwrap_or_else(Uuid::new_v4),
        type_,
        run_time_ticks: runtime_s.map(|s| s * TICKS_PER_SEC),
        media_sources: Some(
            ids.iter()
                .map(|id| api::MediaSourceInfo {
                    id: *id,
                    ..Default::default()
                })
                .collect(),
        ),
        ..Default::default()
    }
}

/// A stream row of `secs` seconds under `parent`, measured by `origin`.
fn measured(
    parent: Option<Uuid>,
    secs: f64,
    origin: Option<api::ProbeOrigin>,
) -> db::Media {
    db::Media {
        title: "Invented.Film.1080p.mkv".to_string(),
        kind: db::MediaKind::Stream,
        parent_id: parent,
        probe_data: Some(api::MediaSourceInfo {
            run_time_ticks: Some((secs * TICKS_PER_SEC as f64).round() as i64),
            remux: Some(api::MediaSourceRemuxInfo {
                provider_info: None,
                source: origin,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A test server's context whose data directory is `dir`, so nothing a
/// test writes or deletes touches a real data directory.
async fn ctx_in(dir: &Path) -> crate::integration_test::TestGuard {
    let (_server, guard) =
        crate::integration_test::new_test_server_with_config(crate::Config {
            data_dir: dir.to_path_buf(),
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            torrent_peer_port: None,
            disable_dht: true,
            disable_startup_tasks: true,
            ..Default::default()
        })
        .await
        .unwrap();
    guard
}

/// A database holding only settings: the switch and the cache size.
async fn settings_db(on: bool, cache_mb: i64) -> sqlx::SqlitePool {
    let pool = db::connect("sqlite::memory:", 10_000)
        .await
        .unwrap();
    db::migrate(&pool)
        .await
        .unwrap();
    save_settings(&pool, on, cache_mb).await;
    pool
}

async fn save_settings(pool: &sqlx::SqlitePool, on: bool, cache_mb: i64) {
    let cfg = api::ServerConfiguration {
        trickplay_while_streaming: Some(on),
        trickplay_cache_size_mb: Some(cache_mb),
        ..Default::default()
    };
    db::Settings::set_config(pool, &cfg)
        .await
        .unwrap();
}

/// A pass over a 7,200 s stream with a set of its own, whose version
/// played is listed by `addon`.
fn addon_plan(root: &Path, stream_id: Uuid, addon: &Addon) -> PassPlan {
    PassPlan {
        played: StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            addon_id: Some(addon.id),
            ..Default::default()
        },
        ..plan(root, stream_id)
    }
}

fn state(sheets: impl IntoIterator<Item = u32>, end: bool) -> SetState {
    SetState {
        sheets: sheets
            .into_iter()
            .collect(),
        end,
    }
}

/// A pass over a film of 940 pictures (sheets 0 to 37) with the start
/// position off.
fn in_order(root: &Path, set_id: Uuid) -> PassPlan {
    PassPlan {
        pictures: 940,
        from_position: false,
        ..plan(root, set_id)
    }
}

/// What the pass of the version played reads, in [`set_plan`].
const PLAYED: &str = "/media/invented-film.mkv";

/// A version of `title` measured at `secs`, cached at a named service
/// and listed at [`LISTED`] bytes, from the addon `addon_id`, listed at
/// `idx`.
fn version(title: Uuid, secs: f64, addon_id: Uuid, idx: i64) -> db::Media {
    db::Media {
        idx: Some(idx),
        stream_info: Some(StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            addon_id: Some(addon_id),
            service_id: Some("svc-cached".to_string()),
            service_cached: Some(true),
            size: Some(LISTED as i64),
            ..Default::default()
        }),
        ..measured(Some(title), secs, Some(api::ProbeOrigin::Ffprobe))
    }
}

fn rules_of(addons: &[&Addon]) -> AddonRules {
    addons
        .iter()
        .map(|addon| (addon.id, AddonRule::new(addon)))
        .collect()
}

fn info_of(row: &mut db::Media) -> &mut StreamInfo {
    row.stream_info
        .as_mut()
        .unwrap()
}

/// Another version listed at [`LISTED`] bytes, cached at a named
/// service, read from `input`.
fn other_source(input: &str) -> PassSource {
    PassSource {
        stream_id: Uuid::new_v4(),
        input: input.to_string(),
        info: StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            service_id: Some("svc-cached".to_string()),
            service_cached: Some(true),
            size: Some(LISTED as i64),
            ..Default::default()
        },
        kind: SourceKind::Other,
    }
}

/// A pass over the version played into `set_id`, with `others` behind it.
fn plan_with(root: &Path, set_id: Uuid, others: &[PassSource]) -> PassPlan {
    PassPlan {
        others: others.to_vec(),
        ..set_plan(root, Uuid::new_v4(), set_id)
    }
}

fn strings(inputs: &[&str]) -> Vec<String> {
    inputs
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// A line no table knows, as an ffmpeg might word a source it gave up on.
const UNKNOWN_LINE: &str =
    "[tls @ 0x9] The specified session has been invalidated for some reason.\n";

/// A run's job for [`read_progress`], writing nowhere.
fn progress_job(dir: &Path) -> PassJob {
    PassJob {
        input: URL.to_string(),
        out_dir: dir.to_path_buf(),
        pid: Arc::new(AtomicU32::new(0)),
        min_free_bytes: 0,
        start_sheet: 0,
        stop_after: None,
        first_sheet_within: None,
        made: Arc::new(AtomicU32::new(0)),
    }
}

/// Two versions of one 7,200 s title, each read from its own input, into
/// one set: version-a's play starts the pass and version-b's joins it
/// while it waits for the slot. Gives version-b's stream id.
async fn join_two_versions(
    passes: &TrickplayPasses,
    root: &Path,
    set_id: Uuid,
    runner: &Arc<FakeRunner>,
    joined: PassPlan,
) -> Uuid {
    let held = passes
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let first = passes
        .begin(
            "play-a",
            PassPlan {
                input: "version-a".to_string(),
                pictures: joined.pictures,
                ..set_plan(root, Uuid::new_v4(), set_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(first, Begin::Queued);
    let version_b = joined.stream_id;
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let second = passes
        .begin("play-b", joined, runner.clone(), always())
        .await;
    assert_eq!(second, Begin::Joined);
    drop(held);
    until(|| !passes.is_queued(set_id)).await;
    version_b
}

/// Version-b's plan, read from its own input into `set_id`.
fn version_b(root: &Path, set_id: Uuid) -> PassPlan {
    PassPlan {
        input: "version-b".to_string(),
        ..set_plan(root, Uuid::new_v4(), set_id)
    }
}

/// A 300 s clip of 64x64 test pictures, one a second and each a key
/// frame, made by the ffmpeg this build runs. False without one.
fn make_clip(path: &Path) -> bool {
    std::process::Command::new(crate::tools::ffmpeg())
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=300:size=64x64:rate=1",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "1",
            "-y",
        ])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Addons kept for seek previews only.

/// A finder that hands back `sources`, with `other_lengths` stored
/// streams of another length, and counts its calls.
#[derive(Default)]
struct FakeFinder {
    sources: Vec<PassSource>,
    other_lengths: usize,
    calls: AtomicUsize,
}

impl PreviewFinder for FakeFinder {
    fn find(&self, _ask: &PreviewAsk) -> impl Future<Output = Found> + Send {
        self.calls
            .fetch_add(1, Ordering::SeqCst);
        std::future::ready(Found {
            sources: self
                .sources
                .clone(),
            other_lengths: self.other_lengths,
        })
    }
}

impl FakeFinder {
    fn calls(&self) -> usize {
        self.calls
            .load(Ordering::SeqCst)
    }
}

/// A stream of `addon`, kept for seek previews only, listed at
/// [`LISTED`] bytes, cached at a named service, read from `input`.
fn preview_source(input: &str, addon: &Addon) -> PassSource {
    let mut source = other_source(input);
    source
        .info
        .addon_id = Some(addon.id);
    source.kind = SourceKind::Preview;
    source
}

/// [`plan_with`], asking the addons kept for seek previews only.
fn preview_plan(root: &Path, set_id: Uuid, others: &[PassSource]) -> PassPlan {
    PassPlan {
        preview: Some(PreviewAsk {
            title_id: Uuid::new_v4(),
            playing: db::Media::default(),
            user_id: Uuid::new_v4(),
            root: root.to_path_buf(),
        }),
        ..plan_with(root, set_id, others)
    }
}

/// Passes whose snapshot holds `dedicated`.
fn passes_with(dedicated: &Addon) -> TrickplayPasses {
    let passes = TrickplayPasses::with_limits(fast());
    passes.set_addon_rules(std::slice::from_ref(dedicated));
    passes
}

// A length read from the header of the version played as it plays.

/// A start delay long enough to see a length arrive part way through it.
const WAIT_DELAY: Duration = Duration::from_millis(1_500);
/// How much later than planned a test's timer may fire.
const SLACK: Duration = Duration::from_millis(250);

/// Passes whose snapshot holds `dedicated`, with [`WAIT_DELAY`].
fn waiting_passes(dedicated: &Addon) -> TrickplayPasses {
    let passes = TrickplayPasses::with_limits(Limits {
        start_delay: WAIT_DELAY,
        length_check: Duration::from_millis(40),
        ..fast()
    });
    passes.set_addon_rules(std::slice::from_ref(dedicated));
    passes
}

/// A version of `title` stored with the title's listed runtime and no
/// measured length.
fn unmeasured(title: Uuid, addon_id: Uuid) -> db::Media {
    let mut row = version(title, RUNTIME as f64, addon_id, 0);
    if let Some(probe) = row
        .probe_data
        .as_mut()
    {
        probe.remux = Some(api::MediaSourceRemuxInfo {
            provider_info: None,
            source: Some(api::ProbeOrigin::FilenameGuess),
        });
    }
    row
}

/// `row` with its length read from its header.
fn learned(row: &db::Media) -> db::Media {
    db::Media {
        probe_data: with_length(
            row.probe_data
                .clone(),
            RUNTIME as i64 * TICKS_PER_SEC,
        ),
        ..row.clone()
    }
}

/// A made-up film stored in `ctx`, with one version listed at the film's
/// runtime, its length measured or not.
async fn film_and_version(
    ctx: &AppContext,
    tmdb: i64,
    measured: bool,
) -> (db::Media, db::Media) {
    let mut film = db::Media {
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Movie,
        external_ids: db::ExternalIds {
            tmdb: Some(tmdb),
            ..Default::default()
        },
        runtime: Some(RUNTIME as i64),
        ..Default::default()
    };
    film.save(&ctx.db)
        .await
        .unwrap();
    let origin = if measured {
        api::ProbeOrigin::Ffprobe
    } else {
        api::ProbeOrigin::FilenameGuess
    };
    let mut stream = db::Media {
        title: "Invented.Film.1080p.mkv".to_string(),
        kind: db::MediaKind::Stream,
        parent_id: Some(film.id),
        stream_info: Some(StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            ..Default::default()
        }),
        probe_data: Some(api::MediaSourceInfo {
            run_time_ticks: Some(RUNTIME as i64 * TICKS_PER_SEC),
            remux: Some(api::MediaSourceRemuxInfo {
                provider_info: None,
                source: Some(origin),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    stream
        .save(&ctx.db)
        .await
        .unwrap();
    (film, stream)
}

fn play_of_version(film: &db::Media, stream: &db::Media) -> PlayStart {
    PlayStart {
        play_session_id: "play-a".to_string(),
        item_id: film.id,
        media_source_id: Some(stream.id),
        device_id: "device-a".to_string(),
        user_id: Uuid::new_v4(),
    }
}

impl TrickplayPasses {
    /// [`Self::begin_with`] with a finder that finds nothing.
    async fn begin<R, W>(
        &self,
        play_session_id: &str,
        plan: PassPlan,
        runner: Arc<R>,
        watch: W,
    ) -> Begin
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static,
    {
        self.begin_with(play_session_id, plan, runner, watch, &NoFinder)
            .await
    }

    /// Until when an addon kept for seek previews only is not asked again
    /// for a title.
    pub(crate) fn asked_until(
        &self,
        title_id: Uuid,
        addon_id: Uuid,
    ) -> Option<Instant> {
        self.lock()
            .asked
            .get(&(title_id, addon_id))
            .map(|asked| asked.until)
    }

    fn registration_count(&self) -> usize {
        self.lock()
            .by_session
            .len()
    }

    fn has_pass(&self, set_id: Uuid) -> bool {
        self.lock()
            .by_set
            .contains_key(&set_id)
    }

    fn is_queued(&self, set_id: Uuid) -> bool {
        self.lock()
            .by_set
            .get(&set_id)
            .is_some_and(|p| {
                p.cancel
                    .is_some()
            })
    }

    fn is_waiting(&self, set_id: Uuid) -> bool {
        self.lock()
            .by_set
            .get(&set_id)
            .is_some_and(|p| p.waiting)
    }
}

/// A finder that finds nothing, for a pass with no addon kept for seek
/// previews only.
struct NoFinder;

impl PreviewFinder for NoFinder {
    fn find(&self, _ask: &PreviewAsk) -> impl Future<Output = Found> + Send {
        std::future::ready(Found::default())
    }
}
