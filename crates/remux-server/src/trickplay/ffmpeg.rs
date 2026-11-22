//! Running ffmpeg for one run of a pass, and judging how it ended.

use super::*;

/// What ffmpeg reads: a local file by its path, anything else through this
/// server's own stream proxy, never the source's own URL.
pub fn pass_input(info: &StreamInfo, stream_id: Uuid, port: u16) -> String {
    match &info.descriptor {
        StreamDescriptor::Local(path) => path
            .to_string_lossy()
            .into_owned(),
        _ => crate::stream::proxy_input(stream_id, port),
    }
}

/// ffmpeg's arguments for one run, starting at sheet `start_sheet`. A run
/// that starts part way seeks there (`-ss`, before the input) and numbers its
/// sheets from there; `start_time=0` keeps its pictures on the same
/// 10-second grid as a read from the start.
pub fn pass_args(input: &str, out_dir: &Path, start_sheet: u32) -> Vec<String> {
    let mut args: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-threads",
        "1",
        "-filter_threads",
        "1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(
        crate::playback::engine::ffmpeg_http_input_args(input)
            .iter()
            .map(|s| s.to_string()),
    );
    let filter = format!(
        "fps=1/{INTERVAL_SECS}:start_time=0,\
         scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,\
         tile={TILE_WIDTH}x{TILE_HEIGHT}"
    );
    // ffmpeg reads the whole name as a pattern: a `%` in the folder's path is
    // written `%%` so it stays a plain character.
    let folder = out_dir
        .to_string_lossy()
        .replace('%', "%%");
    let output = Path::new(&folder)
        .join("%d.jpg")
        .to_string_lossy()
        .into_owned();
    args.extend(["-skip_frame".to_string(), "nokey".to_string()]);
    if start_sheet > 0 {
        args.extend([
            "-ss".to_string(),
            (u64::from(start_sheet) * SHEET_SECS).to_string(),
        ]);
    }
    args.extend([
        "-i".to_string(),
        input.to_string(),
        "-an".to_string(),
        "-sn".to_string(),
        "-dn".to_string(),
        "-vf".to_string(),
        filter,
        "-q:v".to_string(),
        "5".to_string(),
        "-f".to_string(),
        "image2".to_string(),
        "-atomic_writing".to_string(),
        "1".to_string(),
        "-start_number".to_string(),
        start_sheet.to_string(),
        output,
        "-progress".to_string(),
        "pipe:1".to_string(),
        "-nostats".to_string(),
    ]);
    args
}

/// How a run ended, judged in this order (see [`run_end`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunEnd {
    /// It made the stretch it was given.
    Goal,
    /// Stopped from outside, or by low disk.
    Cancelled,
    /// It read to the end of the file.
    End,
    /// It started part way and made no sheet within its wait.
    NoJump,
    /// It started part way and found the file already over.
    Nothing,
    /// It reached the end of the file with exit 0, but ffmpeg wrote a line
    /// none of the tables below knows (see [`unknown_line`]). That may be a
    /// source given up on in words the tables lack, so the last sheet may be
    /// partly filled or missing: the end is not marked.
    Unclean,
    Failed,
}

// The tables below were written from ffmpeg's source (fftools, and
// libavformat's http and matroska code) and from runs of the jellyfin-ffmpeg
// this server's image installs (`docker/Dockerfile`; the desktop app's
// `FFMPEG_MAJOR`). Another ffmpeg may word its lines differently; a line a
// table lacks then never marks the end of a file (see `RunEnd::Unclean`).

/// The exit status ffmpeg gives when its input ended before any picture
/// reached the sheet encoder, as when a run starts past the end of the file:
/// the encoder cannot be opened without a picture, so it ends with
/// AVERROR(EINVAL), -22, which is 234 on Unix (its low byte) and -22 on
/// Windows, and writes no file. An ffmpeg that ends such a run with exit 0
/// is read by its status.
pub(super) const EMPTY_END_STATUS: [i32; 2] = [234, -22];

/// What ffmpeg writes at `-loglevel error` with [`EMPTY_END_STATUS`] when
/// no picture reached the output before its input ended: the encoder could
/// not open (fftools/ffmpeg_enc.c), or nothing was muxed (fftools/ffmpeg_mux.c
/// of_write_trailer); both end with AVERROR(EINVAL).
pub(super) const EMPTY_END_LINES: [&str; 2] = [
    "Could not open encoder before EOF",
    "Nothing was written into output file",
];

/// What ffmpeg writes at `-loglevel error` at a clean end of a file read over
/// HTTP, from a source that closes the request at the file's very end.
pub(super) const HARMLESS_END_LINES: [&str; 2] =
    ["Error reading HTTP response", "File ended prematurely"];

/// What ffmpeg writes at `-loglevel error` when its input failed rather than
/// ended: a read error it gave up on after its reconnects (which ffmpeg then
/// treats as the end of the input, so a run that wrote nothing exits with
/// [`EMPTY_END_STATUS`] all the same), an answer cut short of its length, an
/// HTTP error status, or a connection that failed. "Error reading HTTP
/// response" is not one (see [`HARMLESS_END_LINES`]).
pub(super) const SOURCE_ERRORS: [&str; 10] = [
    "Error during demuxing",
    "Read error at pos",
    "Stream ends prematurely",
    "Failed to reconnect",
    "Server returned",
    "Connection refused",
    "Connection reset",
    "Connection timed out",
    "Input/output error",
    "Error opening input",
];

/// The lines of [`SOURCE_ERRORS`] that ffmpeg writes only once it has given
/// up on its input: its reconnects ran out and the read error reached the
/// command line ("Error during demuxing: Input/output error", after the
/// demuxer's own "Read error", which some write as "Read error at pos"), or
/// a reconnect itself failed.
/// "Stream ends prematurely" is not one: ffmpeg writes it before every
/// reconnect, also one that then works. Nor is a bare "Error during
/// demuxing", which a damaged end of a whole file can give too.
pub(super) const GAVE_UP_ERRORS: [&str; 3] = [
    "Read error at pos",
    "Input/output error",
    "Failed to reconnect",
];

/// Whether a run ended by itself at the end of its input having written no
/// sheet, with no sign that the source failed: exit 0, or
/// [`EMPTY_END_STATUS`] with one of [`EMPTY_END_LINES`], and none of
/// [`SOURCE_ERRORS`] in what ffmpeg wrote.
pub(super) fn ended_empty(result: &RunResult) -> bool {
    let by_itself = !(result.cancelled || result.reached_goal || result.no_first_sheet);
    let clean_status = result.exit_ok
        || (result
            .exit_code
            .is_some_and(|code| EMPTY_END_STATUS.contains(&code))
            && EMPTY_END_LINES
                .iter()
                .any(|line| {
                    result
                        .stderr
                        .contains(line)
                }));
    by_itself && result.made_sheets == 0 && clean_status && !source_failed(result)
}

/// Whether what ffmpeg wrote has a sign that its source failed (see
/// [`SOURCE_ERRORS`]).
pub(super) fn source_failed(result: &RunResult) -> bool {
    SOURCE_ERRORS
        .iter()
        .any(|error| {
            result
                .stderr
                .contains(error)
        })
}

/// Whether what ffmpeg wrote shows it gave up on its source (see
/// [`GAVE_UP_ERRORS`]).
pub(super) fn source_gave_up(result: &RunResult) -> bool {
    GAVE_UP_ERRORS
        .iter()
        .any(|error| {
            result
                .stderr
                .contains(error)
        })
}

/// Every line the tables know: [`SOURCE_ERRORS`] (which hold
/// [`GAVE_UP_ERRORS`]), [`EMPTY_END_LINES`] and [`HARMLESS_END_LINES`].
pub(super) fn known_lines() -> impl Iterator<Item = &'static str> {
    let tables: [&'static [&'static str]; 3] =
        [&SOURCE_ERRORS, &EMPTY_END_LINES, &HARMLESS_END_LINES];
    tables
        .into_iter()
        .flatten()
        .copied()
}

/// Whether ffmpeg wrote a line that holds none of [`known_lines`].
pub(super) fn unknown_line(result: &RunResult) -> bool {
    result
        .stderr
        .lines()
        .any(|line| {
            !line
                .trim()
                .is_empty()
                && !known_lines().any(|known| line.contains(known))
        })
}

/// What a run keeps of ffmpeg's stderr, taken line by line as it arrives, so
/// a flood of lines holds a bounded amount: the first line holding each of
/// [`known_lines`], however early it came, so the judging of the run
/// ([`source_failed`], [`source_gave_up`], [`ended_empty`]) sees it; the
/// first line holding none of them, so [`unknown_line`] sees one; and the
/// last [`STDERR_TAIL_BYTES`] of the rest, for the log.
#[derive(Debug, Default)]
pub(super) struct StderrKept {
    pub(super) noted: Vec<String>,
    pub(super) seen: HashSet<&'static str>,
    pub(super) unknown_seen: bool,
    pub(super) tail: std::collections::VecDeque<String>,
    pub(super) tail_bytes: usize,
}

impl StderrKept {
    pub(super) fn push(&mut self, line: &str) {
        let line = line.trim_end();
        if line
            .trim()
            .is_empty()
        {
            return;
        }
        let matched: Vec<&'static str> = known_lines()
            .filter(|known| line.contains(known))
            .collect();
        let noted = if matched.is_empty() {
            !std::mem::replace(&mut self.unknown_seen, true)
        } else {
            // Every match is recorded, so no short-circuit here.
            matched
                .into_iter()
                .filter(|known| {
                    self.seen
                        .insert(known)
                })
                .count()
                > 0
        };
        if noted {
            self.noted
                .push(line.to_string());
            return;
        }
        self.tail_bytes += line.len() + 1;
        self.tail
            .push_back(line.to_string());
        while self.tail_bytes > STDERR_TAIL_BYTES {
            let Some(old) = self
                .tail
                .pop_front()
            else {
                break;
            };
            self.tail_bytes -= old.len() + 1;
        }
    }

    /// The lines kept, the noted ones first, one a line.
    pub(super) fn text(self) -> String {
        let mut out = String::new();
        for line in self
            .noted
            .iter()
            .chain(
                self.tail
                    .iter(),
            )
        {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// Reads ffmpeg's stderr to its end into a [`StderrKept`]. A line ends at a
/// newline or a carriage return; only its first [`STDERR_LINE_BYTES`] are
/// kept, so a line that never ends holds no more than that.
pub(super) async fn read_stderr<S>(mut stderr: S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut kept = StderrKept::default();
    let mut line: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stderr
            .read(&mut buf)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &buf[..n] {
            if byte == b'\n' || byte == b'\r' {
                kept.push(&String::from_utf8_lossy(&line));
                line.clear();
            } else if line.len() < STDERR_LINE_BYTES {
                line.push(byte);
            }
        }
    }
    if !line.is_empty() {
        kept.push(&String::from_utf8_lossy(&line));
    }
    kept.text()
}

/// Judges a run. `End` needs ffmpeg to have ended by itself, and either exit
/// 0 with the count's last sheet or the sheet before it (a last sheet may
/// never come) and no sign that it gave up on its source (a source cut off
/// there exits 0 too, having flushed a partly filled sheet; see
/// [`GAVE_UP_ERRORS`]), or no sheet from a run that started at the last (see
/// [`ended_empty`]). Such a run with exit 0 and a line the tables do not know
/// is `Unclean` instead. A run that ends short of that by itself is a
/// failure, as a source cut off part way also exits 0, except one that
/// started part way and ended empty: the file ends before that place.
pub(super) fn run_end(result: &RunResult, run: Run, pictures: u64) -> RunEnd {
    let last = last_sheet(pictures);
    let start = u64::from(run.start);
    if result.reached_goal {
        return RunEnd::Goal;
    }
    if result.cancelled {
        return RunEnd::Cancelled;
    }
    let empty = ended_empty(result);
    let reached = match result.made_sheets {
        0 => empty && start >= last,
        made => {
            let highest = start + made - 1;
            result.exit_ok
                && highest >= last.saturating_sub(1)
                && !source_gave_up(result)
        }
    };
    if reached {
        // With `EMPTY_END_STATUS`, the other lines ffmpeg writes are those
        // of a run that found no picture (see `ended_empty`).
        if result.exit_ok && unknown_line(result) {
            return RunEnd::Unclean;
        }
        return RunEnd::End;
    }
    if result.no_first_sheet {
        return RunEnd::NoJump;
    }
    if empty && start > 0 {
        return RunEnd::Nothing;
    }
    RunEnd::Failed
}

/// One sheet line of ffmpeg's `-progress` output.
pub(super) fn progress_frame(line: &str) -> Option<u64> {
    line.strip_prefix("frame=")?
        .trim()
        .parse()
        .ok()
}

/// Free bytes on the filesystem holding `path`.
#[cfg(unix)]
pub(super) fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(
        path.as_os_str()
            .as_bytes(),
    )
    .ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` points to
    // writable memory of the right type; it is read only after success.
    let stat = unsafe {
        if libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    #[allow(clippy::useless_conversion)]
    let free = u64::from(stat.f_bavail) * u64::from(stat.f_frsize);
    Some(free)
}

/// Not known off Unix (the Windows build): the 2 GB free-disk stop
/// ([`MIN_FREE_BYTES`]) then never fires, and the admin page says it holds
/// on Linux and macOS only.
#[cfg(not(unix))]
pub(super) fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(unix)]
pub(super) fn lower_priority(pid: u32) {
    // SAFETY: plain system call on a process this server just started.
    let ret = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, 19) };
    if ret != 0 {
        debug!(
            error = %std::io::Error::last_os_error(),
            "could not lower trick play pass priority"
        );
    }
}

/// Does nothing off Unix (the Windows build): ffmpeg runs at normal
/// priority there, and the admin page says low priority on Linux and macOS
/// only.
#[cfg(not(unix))]
pub(super) fn lower_priority(_pid: u32) {}

#[cfg(unix)]
pub(super) fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    // SAFETY: plain system call; the pid is that of a pass this server started.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Does nothing off Unix (the Windows build): there `kill_all` has no last
/// resort for a pass that does not stop within [`KILL_WAIT`], and only
/// `kill_on_drop` and the stop each run is sent stand behind it.
#[cfg(not(unix))]
pub(super) fn kill_pid(_pid: u32) {}

/// What a runner is given: one run of a pass.
#[derive(Debug, Clone)]
pub(crate) struct PassJob {
    pub input: String,
    pub out_dir: PathBuf,
    /// Set to the child's pid once it runs.
    pub pid: Arc<AtomicU32>,
    pub min_free_bytes: u64,
    /// The sheet the run starts at (see [`pass_args`]).
    pub start_sheet: u32,
    /// Stop once this many sheets are made, if set.
    pub stop_after: Option<u64>,
    /// Give up when no sheet is made within this long, if set: only for a
    /// run that starts part way.
    pub first_sheet_within: Option<Duration>,
    /// The sheets made so far, kept up to date while it runs.
    pub made: Arc<AtomicU32>,
}

/// How a runner ended.
#[derive(Debug, Clone, Default)]
pub(crate) struct RunResult {
    pub exit_ok: bool,
    /// ffmpeg's exit status, when it exited by itself (see [`ended_empty`]).
    pub exit_code: Option<i32>,
    /// The last `frame=` value of `-progress`, which counts sheets.
    pub made_sheets: u64,
    /// Stopped by a cancel or by low disk, not by its own end.
    pub cancelled: bool,
    /// With `cancelled`: ffmpeg's output had already ended before the stop
    /// came, so it may have flushed a partly filled last sheet.
    pub ended_itself: bool,
    /// Stopped after making `stop_after` sheets, every one of them whole.
    pub reached_goal: bool,
    /// Stopped because `first_sheet_within` passed with no sheet.
    pub no_first_sheet: bool,
    /// What ffmpeg wrote to stderr, as [`StderrKept`] keeps it.
    pub stderr: String,
}

/// Makes the pictures of one run; ffmpeg in production, a fake in tests.
/// It must stop, and wait for its process to exit, once `cancel` resolves
/// (a stop sent or the sender dropped).
pub(crate) trait PassRunner: Send + Sync + 'static {
    fn run(
        &self,
        job: PassJob,
        cancel: oneshot::Receiver<()>,
    ) -> impl Future<Output = RunResult> + Send;

    /// The size in bytes of the file at `input`, as its source gives it now,
    /// or `None` when it gives none.
    fn size(&self, input: &str) -> impl Future<Output = Option<u64>> + Send;
}

/// Runs ffmpeg at low priority with [`pass_args`].
pub(crate) struct FfmpegRunner;

impl PassRunner for FfmpegRunner {
    fn run(
        &self,
        job: PassJob,
        cancel: oneshot::Receiver<()>,
    ) -> impl Future<Output = RunResult> + Send {
        run_ffmpeg(job, cancel)
    }

    fn size(&self, input: &str) -> impl Future<Output = Option<u64>> + Send {
        source_size(input.to_string())
    }
}

/// The client of [`source_size`], for this server's own stream proxy.
pub(super) static SIZE_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(SIZE_WAIT)
            .build()
            .expect("failed to build the trick play size client")
    });

/// The size of the file at a pass input (see [`pass_input`]): a local file's
/// own, or the total a request for its first byte through the stream proxy
/// is answered with. Only the answer's head is read.
pub(super) async fn source_size(input: String) -> Option<u64> {
    if !input.starts_with("http://") && !input.starts_with("https://") {
        return tokio::fs::metadata(&input)
            .await
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
    }
    let response = SIZE_CLIENT
        .get(&input)
        .header(http::header::RANGE, "bytes=0-0")
        .send()
        .await
        .ok()?;
    let status = response.status();
    if status == http::StatusCode::PARTIAL_CONTENT {
        response
            .headers()
            .get(http::header::CONTENT_RANGE)?
            .to_str()
            .ok()?
            .rsplit_once('/')?
            .1
            .trim()
            .parse()
            .ok()
    } else if status == http::StatusCode::OK {
        response.content_length()
    } else {
        None
    }
}

/// What [`read_progress`] saw of a run.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct RunProgress {
    /// The last `frame=` value, which counts sheets.
    pub(super) made_sheets: u64,
    /// A stop came on `cancel`.
    pub(super) cancelled: bool,
    pub(super) low_disk: bool,
    pub(super) reached_goal: bool,
    pub(super) no_first_sheet: bool,
    /// ffmpeg wrote its last progress block (`progress=end`): its output
    /// had ended, and may hold a partly filled last sheet.
    pub(super) ended: bool,
}

impl RunProgress {
    /// A stop that came after ffmpeg's last progress block: its output had
    /// already ended, so its last sheet may be partly filled and goes (see
    /// [`RunResult::ended_itself`]).
    pub(super) fn ended_before_stop(&self) -> bool {
        self.cancelled && self.ended
    }
}

/// Reads a run's `-progress` lines from `stdout` until ffmpeg closes it, a
/// stop comes on `cancel`, the run reaches its goal, its first sheet is late
/// or the disk runs low. Lines already written are read before a stop is
/// looked at (`biased`), so a stop that comes with ffmpeg's last progress
/// block never hides the count of a last, partly filled sheet.
pub(super) async fn read_progress<B>(
    stdout: B,
    job: &PassJob,
    cancel: &mut oneshot::Receiver<()>,
) -> RunProgress
where
    B: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut seen = RunProgress::default();
    let mut next_check = 10;
    let first_sheet_by = job
        .first_sheet_within
        .map(|wait| tokio::time::Instant::now() + wait);
    let mut lines = stdout.lines();
    loop {
        tokio::select! {
            biased;
            line = lines.next_line() => {
                let Ok(Some(line)) = line else {
                    break;
                };
                // Each block of progress lines ends with `progress=continue`
                // or `progress=end`. A sheet counted in a `continue` block
                // was a full one: a partly filled sheet is only written at
                // the end of the input.
                if line.trim() == "progress=end" {
                    seen.ended = true;
                }
                if line.trim() == "progress=continue"
                    && job
                        .stop_after
                        .is_some_and(|goal| seen.made_sheets >= goal)
                {
                    seen.reached_goal = true;
                    break;
                }
                let Some(n) = progress_frame(&line) else {
                    continue;
                };
                seen.made_sheets = n;
                job.made
                    .store(u32::try_from(n).unwrap_or(u32::MAX), Ordering::SeqCst);
                if seen.made_sheets >= next_check {
                    next_check = seen.made_sheets + 10;
                    let free = free_bytes(&job.out_dir);
                    if free.is_some_and(|b| b < job.min_free_bytes) {
                        seen.low_disk = true;
                        break;
                    }
                }
            }
            _ = &mut *cancel => {
                seen.cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(
                first_sheet_by.unwrap_or_else(tokio::time::Instant::now)
            ), if first_sheet_by.is_some() && seen.made_sheets == 0 => {
                seen.no_first_sheet = true;
                break;
            }
        }
    }
    seen
}

pub(super) async fn run_ffmpeg(
    job: PassJob,
    mut cancel: oneshot::Receiver<()>,
) -> RunResult {
    let mut cmd = tokio::process::Command::new(crate::tools::ffmpeg());
    cmd.hide_console();
    cmd.kill_on_drop(true);
    cmd.args(pass_args(&job.input, &job.out_dir, job.start_sheet));
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return RunResult {
                stderr: format!("failed to start ffmpeg: {e}"),
                ..Default::default()
            };
        }
    };
    if let Some(pid) = child.id() {
        job.pid
            .store(pid, Ordering::SeqCst);
        lower_priority(pid);
    }
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_stderr(stderr)));

    let seen = match child
        .stdout
        .take()
    {
        Some(stdout) => {
            read_progress(tokio::io::BufReader::new(stdout), &job, &mut cancel).await
        }
        None => RunProgress::default(),
    };
    let mut ended_itself = seen.ended_before_stop();
    let RunProgress {
        made_sheets,
        mut cancelled,
        low_disk,
        reached_goal,
        no_first_sheet,
        ..
    } = seen;

    let mut exit_code = None;
    let exit_ok = if cancelled || low_disk || reached_goal || no_first_sheet {
        let _ = child.start_kill();
        let _ = child
            .wait()
            .await;
        false
    } else {
        tokio::select! {
            status = child.wait() => {
                exit_code = status
                    .as_ref()
                    .ok()
                    .and_then(|s| s.code());
                status.is_ok_and(|s| s.success())
            }
            _ = &mut cancel => {
                cancelled = true;
                ended_itself = true;
                let _ = child.start_kill();
                let _ = child
                    .wait()
                    .await;
                false
            }
        }
    };
    job.pid
        .store(0, Ordering::SeqCst);
    if low_disk {
        warn!(
            "stopping trick play pictures: less than {} GB of disk is free",
            job.min_free_bytes / (1024 * 1024 * 1024)
        );
    }
    let stderr = match stderr_task {
        Some(task) => task
            .await
            .unwrap_or_default(),
        None => String::new(),
    };
    RunResult {
        exit_ok,
        exit_code,
        made_sheets,
        cancelled: cancelled || low_disk,
        ended_itself,
        reached_goal,
        no_first_sheet,
        stderr,
    }
}
